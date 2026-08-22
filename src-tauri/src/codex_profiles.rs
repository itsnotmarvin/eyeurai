//! Provider-managed Codex profiles for independently refreshable accounts.
//!
//! Every profile gets its own `CODEX_HOME`. The official Codex app-server owns
//! the OAuth browser flow, persists and rotates its own tokens, and exposes
//! account/rate-limit metadata over JSON-RPC. EyeUrAI never receives a raw
//! access token or refresh token for these profiles.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::models::{AccountDescriptor, CredentialRef, ProviderId};
use crate::profile_store::ProfileStore;
use crate::providers::error::{scrub, ProviderError};

pub const LOGIN_EVENT: &str = "eyeurai://codex-profile-login";
const PROFILE_DIRECTORY: &str = "codex-profiles-v1";
const AUTH_FILE: &str = "auth.json";
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const RPC_ENV_ALLOWLIST: [&str; 15] = [
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "SystemRoot",
    "SystemDrive",
    "WINDIR",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    // npm installs Codex as a shim that re-launches Node from PATH, and
    // Windows `.cmd` shims need the command interpreter. These variables carry
    // no credentials, proxies, or endpoint overrides.
    "PATH",
    "COMSPEC",
    "PATHEXT",
];
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLoginStarted {
    pub profile_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLoginEvent {
    pub profile_id: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct ManagedCodexAccount {
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub rate_limits: Value,
}

struct RpcProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

fn store(app_data_dir: &Path) -> ProfileStore {
    ProfileStore::new(app_data_dir, PROFILE_DIRECTORY)
}

/// Rebuild the store a profile belongs to from the profile path itself; the
/// path is fully re-validated before use.
fn store_for_profile(profile_home: &Path) -> Result<ProfileStore, ProviderError> {
    let app_data_dir = profile_home
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| ProviderError::forbidden("the Codex profile path has no parent"))?;
    let store = store(app_data_dir);
    store.validate_profile_home(profile_home)?;
    Ok(store)
}

pub fn discover_descriptors(app_data_dir: &Path) -> Result<Vec<AccountDescriptor>, ProviderError> {
    let mut descriptors = Vec::new();
    for (profile_id, profile_home) in store(app_data_dir).profiles()? {
        let auth_path = profile_home.join(AUTH_FILE);
        let auth_is_regular_file = fs::symlink_metadata(&auth_path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false);
        if !auth_is_regular_file {
            continue;
        }

        descriptors.push(
            AccountDescriptor::new(
                ProviderId::Codex,
                format!("codex-profile:{profile_id}"),
                CredentialRef::CodexCli {
                    path: Some(auth_path.to_string_lossy().into_owned()),
                },
            )
            .with_label("EyeUrAI Codex profile")
            .with_option("codex_home", profile_home.to_string_lossy().into_owned()),
        );
    }
    descriptors.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(descriptors)
}

pub async fn start_login(
    app: AppHandle,
    app_data_dir: &Path,
) -> Result<CodexLoginStarted, ProviderError> {
    let store = store(app_data_dir);
    store.ensure_root()?;
    let (profile_id, profile_home) = store.create_profile()?;

    let result = start_login_in_profile(&profile_home).await;
    let (mut rpc, login_id, auth_url) = match result {
        Ok(result) => result,
        Err(error) => {
            store.remove_incomplete(&profile_home);
            return Err(error);
        }
    };
    let login_guard = store.lock_profile_access(&profile_home).await?;

    if let Err(error) = open_external_url(&auth_url) {
        stop_rpc(&mut rpc).await;
        store.remove_incomplete(&profile_home);
        return Err(error);
    }

    let event_profile_id = profile_id.clone();
    tokio::spawn(async move {
        let _profile_guard = login_guard;
        let completion = tokio::time::timeout(
            LOGIN_TIMEOUT,
            wait_for_login_completion(&mut rpc.lines, &login_id),
        )
        .await;

        let (success, message) = match completion {
            Ok(Ok(())) => match verify_completed_login(&mut rpc, &profile_home).await {
                Ok(()) => (true, None),
                Err(error) => (false, Some(error.message)),
            },
            Ok(Err(error)) => (false, Some(error.message)),
            Err(_) => (
                false,
                Some("Codex sign-in timed out. Start it again from EyeUrAI.".to_string()),
            ),
        };

        stop_rpc(&mut rpc).await;
        if !success {
            store.remove_incomplete(&profile_home);
        }
        let _ = app.emit(
            LOGIN_EVENT,
            CodexLoginEvent {
                profile_id: event_profile_id,
                success,
                message,
            },
        );
    });

    Ok(CodexLoginStarted { profile_id })
}

/// Read one isolated profile through the official Codex app-server. Codex
/// handles refresh-token rotation inside this profile's own `CODEX_HOME`.
pub async fn read_managed_account(
    profile_home: &Path,
) -> Result<ManagedCodexAccount, ProviderError> {
    let store = store_for_profile(profile_home)?;
    let _guard = store.lock_profile_access(profile_home).await?;
    let mut rpc = start_rpc(profile_home).await?;

    let result = read_managed_account_inner(&mut rpc).await;
    stop_rpc(&mut rpc).await;
    result
}

async fn read_managed_account_inner(
    rpc: &mut RpcProcess,
) -> Result<ManagedCodexAccount, ProviderError> {
    send_request(
        &mut rpc.stdin,
        2,
        "account/read",
        // This is an isolated, provider-owned profile, so the app-server must
        // refresh and persist its own token. The user's default terminal
        // profile never enters this code path.
        json!({ "refreshToken": true }),
    )
    .await?;
    let account_response = read_response(&mut rpc.lines, 2, RPC_TIMEOUT).await?;
    let account = account_response
        .get("result")
        .and_then(|result| result.get("account"))
        .filter(|account| !account.is_null())
        .ok_or_else(|| {
            ProviderError::credentials_missing("this EyeUrAI Codex profile is not signed in")
                .with_remediation("Add the Codex account again in EyeUrAI settings.")
        })?;

    let account_type = account.get("type").and_then(Value::as_str);
    if account_type != Some("chatgpt") {
        return Err(ProviderError::unsupported(
            "this Codex profile is not using a ChatGPT subscription login",
        ));
    }
    let email = account
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let account_id = account
        .get("accountId")
        .or_else(|| account.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let plan = account
        .get("planType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    send_request(&mut rpc.stdin, 3, "account/rateLimits/read", json!({})).await?;
    let limits_response = read_response(&mut rpc.lines, 3, RPC_TIMEOUT).await?;
    let rate_limits = limits_response
        .get("result")
        .and_then(|result| result.get("rateLimits"))
        .cloned()
        .ok_or_else(|| ProviderError::parse("Codex returned no rate-limit snapshot"))?;

    Ok(ManagedCodexAccount {
        account_id,
        email,
        plan,
        rate_limits,
    })
}

async fn verify_completed_login(
    rpc: &mut RpcProcess,
    profile_home: &Path,
) -> Result<(), ProviderError> {
    let auth_path = profile_home.join(AUTH_FILE);
    let auth_is_regular = fs::symlink_metadata(&auth_path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false);
    if !auth_is_regular {
        return Err(ProviderError::internal(
            "Codex sign-in completed but its credential file was not saved",
        ));
    }

    send_request(
        &mut rpc.stdin,
        3,
        "account/read",
        json!({ "refreshToken": true }),
    )
    .await?;
    let response = read_response(&mut rpc.lines, 3, RPC_TIMEOUT).await?;
    let account_type = response
        .get("result")
        .and_then(|result| result.get("account"))
        .and_then(|account| account.get("type"))
        .and_then(Value::as_str);
    if account_type != Some("chatgpt") {
        return Err(ProviderError::credentials_missing(
            "Codex sign-in did not produce a readable ChatGPT account",
        ));
    }
    send_request(&mut rpc.stdin, 4, "account/rateLimits/read", json!({})).await?;
    let limits = read_response(&mut rpc.lines, 4, RPC_TIMEOUT).await?;
    let has_limits = limits
        .get("result")
        .and_then(|result| result.get("rateLimits"))
        .is_some_and(Value::is_object);
    if !has_limits {
        return Err(ProviderError::parse(
            "Codex sign-in completed but usage could not be read",
        ));
    }
    Ok(())
}

async fn stop_rpc(rpc: &mut RpcProcess) {
    let _ = rpc.child.kill().await;
    let _ = rpc.child.wait().await;
}

async fn start_login_in_profile(
    profile_home: &Path,
) -> Result<(RpcProcess, String, String), ProviderError> {
    let mut rpc = start_rpc(profile_home).await?;
    let result = start_login_request(&mut rpc).await;
    match result {
        Ok((login_id, auth_url)) => Ok((rpc, login_id, auth_url)),
        Err(error) => {
            stop_rpc(&mut rpc).await;
            Err(error)
        }
    }
}

async fn start_login_request(rpc: &mut RpcProcess) -> Result<(String, String), ProviderError> {
    send_request(
        &mut rpc.stdin,
        2,
        "account/login/start",
        json!({
            "type": "chatgpt",
            "useHostedLoginSuccessPage": true,
            "appBrand": "codex"
        }),
    )
    .await?;
    let response = read_response(&mut rpc.lines, 2, RPC_TIMEOUT).await?;
    let result = response
        .get("result")
        .ok_or_else(|| rpc_error(&response, "Codex did not start browser sign-in"))?;
    let login_id = result
        .get("loginId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ProviderError::parse("Codex sign-in returned no login identifier"))?;
    let auth_url = result
        .get("authUrl")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ProviderError::parse("Codex sign-in returned no browser URL"))?;
    validate_auth_url(&auth_url)?;
    Ok((login_id, auth_url))
}

async fn start_rpc(profile_home: &Path) -> Result<RpcProcess, ProviderError> {
    store_for_profile(profile_home)?;
    let binary = find_codex_binary()?;
    let inherited_environment: Vec<(String, std::ffi::OsString)> = RPC_ENV_ALLOWLIST
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.to_string(), value)))
        .collect();
    let mut command = Command::new(binary);
    command
        .arg("app-server")
        .arg("--stdio")
        // Do not inherit API/auth endpoint overrides, proxy credentials, or
        // access tokens. The managed profile must talk to Codex defaults;
        // only ordinary OS/runtime paths are restored below.
        .env_clear()
        .env("CODEX_HOME", profile_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command.envs(inherited_environment);
    let mut child = command.spawn().map_err(|_| {
        ProviderError::credentials_missing("the Codex CLI could not be started")
            .with_remediation("Install or update the Codex CLI, then try again.")
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::internal("Codex app-server stdin was unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::internal("Codex app-server stdout was unavailable"))?;
    let mut rpc = RpcProcess {
        child,
        stdin,
        lines: BufReader::new(stdout).lines(),
    };

    let initialization = async {
        send_request(
            &mut rpc.stdin,
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "eyeurai",
                    "title": "EyeUrAI",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
        .await?;
        read_response(&mut rpc.lines, 1, RPC_TIMEOUT).await?;
        send_value(&mut rpc.stdin, &json!({ "method": "initialized" })).await
    }
    .await;
    if let Err(error) = initialization {
        stop_rpc(&mut rpc).await;
        return Err(error);
    }
    Ok(rpc)
}

async fn send_request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), ProviderError> {
    send_value(
        stdin,
        &json!({ "method": method, "id": id, "params": params }),
    )
    .await
}

async fn send_value(stdin: &mut ChildStdin, value: &Value) -> Result<(), ProviderError> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| ProviderError::internal("could not encode a Codex app-server request"))?;
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|_| ProviderError::internal("could not send a request to the Codex app-server"))?;
    stdin
        .flush()
        .await
        .map_err(|_| ProviderError::internal("could not flush the Codex app-server request"))
}

async fn read_response(
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: u64,
    timeout: Duration,
) -> Result<Value, ProviderError> {
    tokio::time::timeout(timeout, async {
        loop {
            let line = lines
                .next_line()
                .await
                .map_err(|_| ProviderError::internal("could not read the Codex app-server"))?
                .ok_or_else(|| ProviderError::internal("the Codex app-server stopped early"))?;
            let value: Value = serde_json::from_str(&line)
                .map_err(|_| ProviderError::parse("Codex app-server returned invalid JSON"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if value.get("error").is_some_and(|error| !error.is_null()) {
                    return Err(rpc_error(&value, "Codex app-server request failed"));
                }
                return Ok(value);
            }
        }
    })
    .await
    .map_err(|_| ProviderError::timeout("the Codex app-server did not respond in time"))?
}

async fn wait_for_login_completion(
    lines: &mut Lines<BufReader<ChildStdout>>,
    login_id: &str,
) -> Result<(), ProviderError> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|_| ProviderError::internal("could not read Codex sign-in status"))?
            .ok_or_else(|| ProviderError::internal("Codex sign-in stopped before completion"))?;
        let value: Value = serde_json::from_str(&line)
            .map_err(|_| ProviderError::parse("Codex sign-in returned invalid status data"))?;
        if value.get("method").and_then(Value::as_str) != Some("account/login/completed") {
            continue;
        }
        let params = value.get("params").unwrap_or(&Value::Null);
        if params.get("loginId").and_then(Value::as_str) != Some(login_id) {
            continue;
        }
        if params.get("success").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let message = params
            .get("error")
            .and_then(Value::as_str)
            .map(scrub)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| "Codex sign-in was not completed.".to_string());
        return Err(ProviderError::unauthorized(message));
    }
}

fn rpc_error(value: &Value, fallback: &str) -> ProviderError {
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(scrub)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    ProviderError::upstream(message)
}

/// Executable names to try inside each candidate directory. On Windows, npm
/// installs a `codex.cmd` shim and the standalone installer ships `codex.exe`;
/// the extensionless `codex` file npm also creates is a POSIX shell shim that
/// Windows cannot execute, so it is deliberately not a candidate there.
fn codex_executable_names(windows: bool) -> &'static [&'static str] {
    if windows {
        &["codex.exe", "codex.cmd"]
    } else {
        &["codex"]
    }
}

fn find_codex_binary() -> Result<PathBuf, ProviderError> {
    let windows = cfg!(target_os = "windows");
    let names = codex_executable_names(windows);

    let mut directories: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".local/bin"));
        directories.push(home.join(".codex/packages/standalone/current/bin"));
    }
    if windows {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            directories.push(PathBuf::from(app_data).join("npm"));
        }
    } else {
        directories.push(PathBuf::from("/opt/homebrew/bin"));
        directories.push(PathBuf::from("/usr/local/bin"));
    }
    // PATH is the normal install location for npm, asdf and several Windows
    // package managers. Standard locations win; PATH remains a compatibility
    // fallback. A same-user process that can replace this executable can also
    // already read the owner-only profile files directly.
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }

    let mut candidates = Vec::new();
    // Keep arbitrary binary overrides out of release builds: the selected
    // process receives CODEX_HOME and therefore has access to that profile's
    // provider-owned credentials.
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("EYEURAI_CODEX_BINARY") {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend(
        directories
            .iter()
            .flat_map(|directory| names.iter().map(move |name| directory.join(name))),
    );

    candidates
        .into_iter()
        .find(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            ProviderError::credentials_missing("EyeUrAI could not find the Codex CLI")
                .with_remediation("Install or update the Codex CLI, then reopen EyeUrAI.")
        })
}

fn validate_auth_url(value: &str) -> Result<(), ProviderError> {
    let url = url::Url::parse(value)
        .map_err(|_| ProviderError::parse("Codex returned an invalid sign-in URL"))?;
    let host = url.host_str().unwrap_or_default();
    let trusted_host = host == "openai.com"
        || host.ends_with(".openai.com")
        || host == "chatgpt.com"
        || host.ends_with(".chatgpt.com");
    if url.scheme() != "https" || !trusted_host {
        return Err(ProviderError::forbidden(
            "Codex returned an untrusted sign-in URL",
        ));
    }
    Ok(())
}

fn open_external_url(value: &str) -> Result<(), ProviderError> {
    validate_auth_url(value)?;
    crate::browser::open_in_browser(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_store::{create_private_directory, unique_profile_id};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            TestDirectory(
                std::env::temp_dir()
                    .join(format!("eyeurai-codex-profiles-{}", unique_profile_id())),
            )
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_only_completed_regular_profiles() {
        let temp = TestDirectory::new();
        let root = store(&temp.0).root().to_path_buf();
        create_private_directory(&root).unwrap();

        let complete = root.join("profile-complete");
        create_private_directory(&complete).unwrap();
        fs::write(complete.join(AUTH_FILE), "{}").unwrap();
        let incomplete = root.join("profile-incomplete");
        create_private_directory(&incomplete).unwrap();
        fs::write(root.join("not-a-profile"), "{}").unwrap();

        let descriptors = discover_descriptors(&temp.0).unwrap();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, "codex-profile:profile-complete");
        assert_eq!(
            descriptors[0].option("codex_home"),
            Some(complete.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn auth_url_validation_rejects_lookalike_and_non_https_hosts() {
        assert!(validate_auth_url("https://auth.openai.com/oauth/authorize").is_ok());
        assert!(validate_auth_url("https://chatgpt.com/auth").is_ok());
        assert!(validate_auth_url("http://auth.openai.com/oauth").is_err());
        assert!(validate_auth_url("https://openai.com.evil.example/oauth").is_err());
    }

    #[test]
    fn rpc_environment_allowlist_excludes_credentials_and_endpoint_overrides() {
        for forbidden in [
            "OPENAI_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "OPENAI_BASE_URL",
            "CODEX_AUTHAPI_BASE_URL",
            "HTTPS_PROXY",
            "SSL_CERT_FILE",
        ] {
            assert!(!RPC_ENV_ALLOWLIST.contains(&forbidden));
        }
    }

    #[test]
    fn rpc_environment_allowlist_keeps_shim_launch_variables() {
        for required in ["PATH", "COMSPEC", "PATHEXT", "SystemRoot"] {
            assert!(RPC_ENV_ALLOWLIST.contains(&required));
        }
    }

    #[test]
    fn windows_codex_candidates_use_executable_extensions() {
        assert_eq!(codex_executable_names(true), ["codex.exe", "codex.cmd"]);
        assert_eq!(codex_executable_names(false), ["codex"]);
    }
}
