//! Provider-managed Codex profiles for independently refreshable accounts.
//!
//! Every profile gets its own `CODEX_HOME`. The official Codex app-server owns
//! the OAuth browser flow, persists and rotates its own tokens, and exposes
//! account/rate-limit metadata over JSON-RPC. EyeUrAI never receives a raw
//! access token or refresh token for these profiles.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::models::{AccountDescriptor, CredentialRef, ProviderId};
use crate::providers::error::{scrub, ProviderError};

pub const LOGIN_EVENT: &str = "eyeurai://codex-profile-login";
const PROFILE_DIRECTORY: &str = "codex-profiles-v1";
const AUTH_FILE: &str = "auth.json";
const PROFILE_LOCK_FILE: &str = ".eyeurai-profile.lock";
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const RPC_ENV_ALLOWLIST: [&str; 11] = [
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "SystemRoot",
    "WINDIR",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
];
static PROFILE_NONCE: AtomicU64 = AtomicU64::new(0);
static PROFILE_LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> = OnceLock::new();

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
    pub email: Option<String>,
    pub plan: Option<String>,
    pub rate_limits: Value,
}

struct RpcProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

struct ProfileAccessGuard {
    _in_process: tokio::sync::OwnedMutexGuard<()>,
    lock_file: File,
}

impl Drop for ProfileAccessGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.lock_file);
    }
}

pub fn discover_descriptors(app_data_dir: &Path) -> Result<Vec<AccountDescriptor>, ProviderError> {
    let root = profiles_root(app_data_dir);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(ProviderError::internal(
                "could not inspect EyeUrAI Codex profiles",
            ))
        }
    };

    let mut descriptors = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(profile_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_profile_id(&profile_id) {
            continue;
        }
        let profile_home = entry.path();
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
    let root = profiles_root(app_data_dir);
    create_private_directory(&root)?;
    let (profile_id, profile_home) = create_profile_directory(&root)?;

    let result = start_login_in_profile(&profile_home).await;
    let (mut rpc, login_id, auth_url) = match result {
        Ok(result) => result,
        Err(error) => {
            remove_incomplete_profile(&root, &profile_home);
            return Err(error);
        }
    };
    let login_guard = lock_profile_access(&profile_home).await?;

    if let Err(error) = open_external_url(&auth_url) {
        stop_rpc(&mut rpc).await;
        remove_incomplete_profile(&root, &profile_home);
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
            remove_incomplete_profile(&root, &profile_home);
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
    validate_profile_home(profile_home)?;
    let _guard = lock_profile_access(profile_home).await?;
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
    validate_profile_home(profile_home)?;
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

fn find_codex_binary() -> Result<PathBuf, ProviderError> {
    let mut candidates = Vec::new();
    // Keep arbitrary binary overrides out of release builds: the selected
    // process receives CODEX_HOME and therefore has access to that profile's
    // provider-owned credentials.
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("EYEURAI_CODEX_BINARY") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/codex"));
        candidates.push(home.join(".codex/packages/standalone/current/bin/codex"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/codex"));
    candidates.push(PathBuf::from("/usr/local/bin/codex"));
    // PATH is the normal install location for npm, asdf and several Windows
    // package managers. Standard locations win; PATH remains a compatibility
    // fallback. A same-user process that can replace this executable can also
    // already read the owner-only profile files directly.
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|directory| directory.join("codex")));
    }

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

fn profiles_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(PROFILE_DIRECTORY)
}

fn create_profile_directory(root: &Path) -> Result<(String, PathBuf), ProviderError> {
    for _ in 0..32 {
        let profile_id = unique_profile_id();
        let profile_home = root.join(&profile_id);
        match fs::create_dir(&profile_home) {
            Ok(()) => {
                #[cfg(unix)]
                fs::set_permissions(&profile_home, fs::Permissions::from_mode(0o700)).map_err(
                    |_| ProviderError::internal("could not secure the new Codex profile"),
                )?;
                return Ok((profile_id, profile_home));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(ProviderError::internal(
                    "could not create a new Codex profile",
                ))
            }
        }
    }
    Err(ProviderError::internal(
        "could not allocate a unique Codex profile",
    ))
}

fn create_private_directory(path: &Path) -> Result<(), ProviderError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(ProviderError::forbidden(
                "the Codex profile directory is not a private directory",
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| {
                ProviderError::internal("could not create the Codex profile directory")
            })?;
            let metadata = fs::symlink_metadata(path).map_err(|_| {
                ProviderError::internal("could not inspect the Codex profile directory")
            })?;
            if !metadata.file_type().is_dir() {
                return Err(ProviderError::forbidden(
                    "the Codex profile directory is not a private directory",
                ));
            }
        }
        Err(_) => {
            return Err(ProviderError::internal(
                "could not inspect the Codex profile directory",
            ))
        }
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| ProviderError::internal("could not secure the Codex profile directory"))?;
    Ok(())
}

fn validate_profile_home(profile_home: &Path) -> Result<(), ProviderError> {
    let profile_id = profile_home
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| valid_profile_id(value))
        .ok_or_else(|| ProviderError::forbidden("the Codex profile identifier is invalid"))?;
    let parent = profile_home
        .parent()
        .ok_or_else(|| ProviderError::forbidden("the Codex profile path has no parent"))?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| {
        ProviderError::credentials_missing("the Codex profile root no longer exists")
    })?;
    if !parent_metadata.file_type().is_dir()
        || parent.file_name().and_then(|value| value.to_str()) != Some(PROFILE_DIRECTORY)
    {
        return Err(ProviderError::forbidden(
            "the Codex profile is outside EyeUrAI profile storage",
        ));
    }
    let metadata = fs::symlink_metadata(profile_home)
        .map_err(|_| ProviderError::credentials_missing("the Codex profile no longer exists"))?;
    if !metadata.file_type().is_dir() {
        return Err(ProviderError::internal(
            "the Codex profile path is not a directory",
        ));
    }
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|_| ProviderError::internal("could not resolve the Codex profile root"))?;
    let canonical_profile = fs::canonicalize(profile_home)
        .map_err(|_| ProviderError::internal("could not resolve the Codex profile"))?;
    if canonical_profile.parent() != Some(canonical_parent.as_path())
        || canonical_profile
            .file_name()
            .and_then(|value| value.to_str())
            != Some(profile_id)
    {
        return Err(ProviderError::forbidden(
            "the Codex profile path could not be trusted",
        ));
    }
    Ok(())
}

fn profile_lock(profile_home: &Path) -> Result<Arc<AsyncMutex<()>>, ProviderError> {
    let canonical = fs::canonicalize(profile_home)
        .map_err(|_| ProviderError::credentials_missing("the Codex profile no longer exists"))?;
    let locks = PROFILE_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .map_err(|_| ProviderError::internal("the Codex profile lock was unavailable"))?;
    Ok(Arc::clone(
        locks
            .entry(canonical)
            .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
    ))
}

async fn lock_profile_access(profile_home: &Path) -> Result<ProfileAccessGuard, ProviderError> {
    validate_profile_home(profile_home)?;
    let in_process = profile_lock(profile_home)?.lock_owned().await;
    let lock_path = profile_home.join(PROFILE_LOCK_FILE);
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(ProviderError::forbidden(
                "the Codex profile lock path could not be trusted",
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ProviderError::internal(
                "could not inspect the Codex profile lock",
            ))
        }
    }
    let lock_file = {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        options
            .open(lock_path)
            .map_err(|_| ProviderError::internal("could not open the Codex profile lock"))?
    };
    let lock_file = tokio::task::spawn_blocking(move || {
        fs2::FileExt::lock_exclusive(&lock_file).map(|_| lock_file)
    })
    .await
    .map_err(|_| ProviderError::internal("the Codex profile lock task stopped"))?
    .map_err(|_| ProviderError::internal("could not lock the Codex profile"))?;
    Ok(ProfileAccessGuard {
        _in_process: in_process,
        lock_file,
    })
}

fn valid_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn unique_profile_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let nonce = PROFILE_NONCE.fetch_add(1, Ordering::Relaxed);
    format!("profile-{nanos:x}-{:x}-{nonce:x}", std::process::id())
}

fn remove_incomplete_profile(root: &Path, profile_home: &Path) {
    if profile_home.parent() == Some(root) {
        let _ = fs::remove_dir_all(profile_home);
    }
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
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg(value);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(value);
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(value);
        command
    };
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| {
            ProviderError::internal("could not open the Codex sign-in page in your browser")
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(ProviderError::internal(
            "the browser could not open the Codex sign-in page",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let root = profiles_root(&temp.0);
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

    #[cfg(unix)]
    #[test]
    fn profile_directories_are_owner_only() {
        let temp = TestDirectory::new();
        let root = profiles_root(&temp.0);
        create_private_directory(&root).unwrap();
        let (_, profile) = create_profile_directory(&root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(profile).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = TestDirectory::new();
        fs::create_dir_all(&temp.0).unwrap();
        let redirect = temp.0.join("redirect");
        fs::create_dir(&redirect).unwrap();
        let root = profiles_root(&temp.0);
        symlink(&redirect, &root).unwrap();

        assert!(create_private_directory(&root).is_err());
    }

    #[test]
    fn repeated_profile_access_uses_the_same_in_process_lock() {
        let temp = TestDirectory::new();
        let root = profiles_root(&temp.0);
        create_private_directory(&root).unwrap();
        let (_, profile) = create_profile_directory(&root).unwrap();

        let first = profile_lock(&profile).unwrap();
        let second = profile_lock(&profile).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn profile_access_holds_an_owner_only_os_lock() {
        let temp = TestDirectory::new();
        let root = profiles_root(&temp.0);
        create_private_directory(&root).unwrap();
        let (_, profile) = create_profile_directory(&root).unwrap();

        let guard = lock_profile_access(&profile).await.unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(profile.join(PROFILE_LOCK_FILE))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_err());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(profile.join(PROFILE_LOCK_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(guard);
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_ok());
        let _ = fs2::FileExt::unlock(&second);
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
}
