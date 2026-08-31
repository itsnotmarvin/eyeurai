//! EyeUrAI-owned Claude profiles for independently refreshable accounts.
//!
//! The terminal's Claude Code login stays a read-only mirror that EyeUrAI
//! never refreshes (Anthropic rotates refresh tokens on use, so refreshing it
//! would invalidate the user's `claude` session). Accounts added inside
//! EyeUrAI are different: each gets its own profile directory with an OAuth
//! grant EyeUrAI obtained itself through Anthropic's official browser
//! sign-in, so EyeUrAI owns that grant and can rotate it safely without ever
//! touching the terminal login.
//!
//! The sign-in is the same authorization-code + PKCE flow Claude Code uses,
//! against Anthropic's first-party OAuth client. EyeUrAI requests only the
//! `user:profile` scope: enough to read usage and identity, deliberately not
//! enough to run inference with the stored token.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::models::{AccountDescriptor, CredentialRef, ProviderId};
use crate::profile_store::ProfileStore;
use crate::providers::credentials::{AccountHint, ResolvedCredential};
use crate::providers::error::ProviderError;
use crate::providers::http::{HttpClient, JsonRequest};
use crate::providers::secret::Secret;

pub const LOGIN_EVENT: &str = "eyeurai://claude-profile-login";
const PROFILE_DIRECTORY: &str = "claude-profiles-v1";
const AUTH_FILE: &str = "auth.json";

/// Anthropic's first-party OAuth client, the one Claude Code itself uses.
/// These are first-party surfaces rather than a documented public API, so
/// they can change; every use fails visibly instead of inventing data.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_ENDPOINT: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";
/// Read-only by construction: `user:profile` can read usage and identity but
/// cannot run inference.
const SCOPES: &str = "user:profile";

const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(20);
/// Refresh this long before nominal expiry so an access token never dies
/// mid-request.
const REFRESH_MARGIN: chrono::Duration = chrono::Duration::minutes(2);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLoginStarted {
    pub profile_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLoginEvent {
    pub profile_id: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The stored grant for one profile. Secrets live only in this owner-only
/// file; identity fields are the non-secret hints shown in the UI.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredGrant {
    version: u32,
    access_token: String,
    refresh_token: String,
    /// Milliseconds since the Unix epoch; 0 when unknown.
    #[serde(default)]
    expires_at_ms: i64,
    /// Scopes actually granted, echoed back on refresh.
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    account_uuid: Option<String>,
    #[serde(default)]
    plan: Option<String>,
}

fn store(app_data_dir: &Path) -> ProfileStore {
    ProfileStore::new(app_data_dir, PROFILE_DIRECTORY)
}

fn store_for_profile(profile_home: &Path) -> Result<ProfileStore, ProviderError> {
    let app_data_dir = profile_home
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| ProviderError::forbidden("the Claude profile path has no parent"))?;
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
                ProviderId::Claude,
                format!("claude-profile:{profile_id}"),
                CredentialRef::ClaudeProfile {
                    path: auth_path.to_string_lossy().into_owned(),
                },
            )
            .with_label("EyeUrAI Claude account")
            .with_option("claude_home", profile_home.to_string_lossy().into_owned()),
        );
    }
    descriptors.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(descriptors)
}

/// Start a browser sign-in for a new isolated Claude profile.
///
/// Returns as soon as the browser is open; completion (success or failure)
/// arrives as a [`LOGIN_EVENT`] carrying the profile id.
pub async fn start_login(
    app: AppHandle,
    app_data_dir: &Path,
    http: HttpClient,
    user_agent: String,
) -> Result<ClaudeLoginStarted, ProviderError> {
    let store = store(app_data_dir);
    store.ensure_root()?;
    let (profile_id, profile_home) = store.create_profile()?;

    // Bind first so the redirect URI in the authorize request is real.
    let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(listener) => listener,
        Err(_) => {
            store.remove_incomplete(&profile_home);
            return Err(ProviderError::internal(
                "could not open a local port for the Claude sign-in redirect",
            ));
        }
    };
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(_) => {
            store.remove_incomplete(&profile_home);
            return Err(ProviderError::internal(
                "could not determine the local Claude sign-in port",
            ));
        }
    };
    let redirect_uri = format!("http://localhost:{port}/callback");

    let pkce = Pkce::generate()?;
    let state = random_urlsafe(32)?;
    let authorize_url = build_authorize_url(&redirect_uri, &pkce.challenge, &state);

    let login_guard = store.lock_profile_access(&profile_home).await?;
    if let Err(error) = crate::browser::open_in_browser(&authorize_url) {
        store.remove_incomplete(&profile_home);
        return Err(error);
    }

    let event_profile_id = profile_id.clone();
    tokio::spawn(async move {
        let _profile_guard = login_guard;
        let completion = tokio::time::timeout(
            LOGIN_TIMEOUT,
            complete_login(
                listener,
                &redirect_uri,
                &pkce,
                &state,
                &profile_home,
                &http,
                &user_agent,
            ),
        )
        .await;

        let (success, message) = match completion {
            Ok(Ok(())) => (true, None),
            Ok(Err(error)) => (false, Some(error.message)),
            Err(_) => (
                false,
                Some("Claude sign-in timed out. Start it again from EyeUrAI.".to_string()),
            ),
        };

        if !success {
            store.remove_incomplete(&profile_home);
        }
        let _ = app.emit(
            LOGIN_EVENT,
            ClaudeLoginEvent {
                profile_id: event_profile_id,
                success,
                message,
            },
        );
    });

    Ok(ClaudeLoginStarted { profile_id })
}

/// Resolve the credential for one profile, refreshing (and persisting the
/// rotated refresh token) when the access token is at or near expiry.
pub async fn resolve_credential(
    profile_home: &Path,
    http: &HttpClient,
    user_agent: &str,
    now: DateTime<Utc>,
) -> Result<ResolvedCredential, ProviderError> {
    let store = store_for_profile(profile_home)?;
    let _guard = store.lock_profile_access(profile_home).await?;

    let mut grant = read_grant(profile_home)?;
    let expires_at = millis_to_utc(grant.expires_at_ms);
    let needs_refresh = expires_at
        .map(|at| at <= now + REFRESH_MARGIN)
        .unwrap_or(false);
    if needs_refresh {
        grant = refresh_grant(grant, profile_home, http, user_agent, now).await?;
    }

    let mut hint = AccountHint {
        subject: grant.account_uuid.clone(),
        email: grant.email.clone(),
        plan: grant.plan.clone(),
        ..AccountHint::default()
    };
    hint.extra
        .insert("managed_by".to_string(), "eyeurai".to_string());

    Ok(ResolvedCredential {
        access_token: Secret::new(&grant.access_token),
        expires_at: millis_to_utc(grant.expires_at_ms),
        hint,
    })
}

async fn refresh_grant(
    grant: StoredGrant,
    profile_home: &Path,
    http: &HttpClient,
    user_agent: &str,
    now: DateTime<Utc>,
) -> Result<StoredGrant, ProviderError> {
    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": grant.refresh_token,
        "client_id": CLIENT_ID,
    });
    if let Some(scope) = &grant.scope {
        body["scope"] = Value::from(scope.clone());
    }

    let response: Value = http
        .post_json(
            &JsonRequest::new(TOKEN_ENDPOINT, user_agent)
                .header("Content-Type", "application/json")
                .timeout(TOKEN_TIMEOUT)
                .max_attempts(1),
            &body,
        )
        .await
        .map_err(|error| {
            if error.kind == crate::models::ProviderErrorKind::Unauthorized
                || error.kind == crate::models::ProviderErrorKind::Forbidden
            {
                ProviderError::credentials_missing(
                    "this EyeUrAI Claude account needs to be signed in again",
                )
                .with_remediation("Add the Claude account again in EyeUrAI settings.")
            } else {
                error
            }
        })?;

    let refreshed = grant_from_token_response(&response, now)?;
    let next = StoredGrant {
        version: 1,
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token,
        expires_at_ms: refreshed.expires_at_ms,
        scope: refreshed.scope.or(grant.scope),
        email: grant.email,
        account_uuid: grant.account_uuid,
        plan: grant.plan,
    };
    write_grant(profile_home, &next)?;
    Ok(next)
}

// ---------------------------------------------------------------------------
// Login internals
// ---------------------------------------------------------------------------

struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn generate() -> Result<Pkce, ProviderError> {
        let verifier = random_urlsafe(64)?;
        Ok(Pkce {
            challenge: challenge_for(&verifier),
            verifier,
        })
    }
}

fn challenge_for(verifier: &str) -> String {
    base64_url(&Sha256::digest(verifier.as_bytes()))
}

fn random_urlsafe(bytes: usize) -> Result<String, ProviderError> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer)
        .map_err(|_| ProviderError::internal("could not gather sign-in randomness"))?;
    Ok(base64_url(&buffer))
}

/// Unpadded base64url, as PKCE requires.
fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let indexes = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        let take = match chunk.len() {
            1 => 2,
            2 => 3,
            _ => 4,
        };
        for index in indexes.iter().take(take) {
            out.push(ALPHABET[*index as usize] as char);
        }
    }
    out
}

fn build_authorize_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    let mut url = url::Url::parse(AUTHORIZE_ENDPOINT).expect("authorize endpoint is a valid URL");
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    url.to_string()
}

#[allow(clippy::too_many_arguments)]
async fn complete_login(
    listener: TcpListener,
    redirect_uri: &str,
    pkce: &Pkce,
    state: &str,
    profile_home: &Path,
    http: &HttpClient,
    user_agent: &str,
) -> Result<(), ProviderError> {
    let code = wait_for_callback(listener, state).await?;

    let response: Value = http
        .post_json(
            &JsonRequest::new(TOKEN_ENDPOINT, user_agent)
                .header("Content-Type", "application/json")
                .timeout(TOKEN_TIMEOUT)
                .max_attempts(1),
            &json!({
                "grant_type": "authorization_code",
                "code": code,
                "redirect_uri": redirect_uri,
                "code_verifier": pkce.verifier,
                "client_id": CLIENT_ID,
                "state": state,
            }),
        )
        .await?;
    let now = Utc::now();
    let tokens = grant_from_token_response(&response, now)?;

    // Identity is best-effort; usage must be readable or the login failed.
    let bearer = Secret::new(&tokens.access_token);
    let profile = http
        .get_json::<Value>(
            &JsonRequest::new("https://api.anthropic.com/api/oauth/profile", user_agent)
                .bearer(&bearer)
                .header("anthropic-beta", "oauth-2025-04-20")
                .timeout(TOKEN_TIMEOUT)
                .max_attempts(1),
        )
        .await
        .ok()
        .map(|value| crate::providers::claude::parse_profile(&value));

    let usage = http
        .get_json::<Value>(
            &JsonRequest::new("https://api.anthropic.com/api/oauth/usage", user_agent)
                .bearer(&bearer)
                .header("anthropic-beta", "oauth-2025-04-20")
                .timeout(TOKEN_TIMEOUT),
        )
        .await?;
    crate::providers::claude::parse_usage(&usage, now).map_err(|_| {
        ProviderError::parse("Claude sign-in completed but usage could not be read")
    })?;

    let grant = StoredGrant {
        version: 1,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at_ms: tokens.expires_at_ms,
        scope: tokens.scope,
        email: profile.as_ref().and_then(|p| p.email.clone()),
        account_uuid: profile.as_ref().and_then(|p| p.account_id.clone()),
        plan: profile.as_ref().and_then(|p| p.plan.clone()),
    };
    write_grant(profile_home, &grant)
}

/// Accept loopback connections until the OAuth redirect with the expected
/// `state` arrives, then answer it with a small completion page.
async fn wait_for_callback(listener: TcpListener, state: &str) -> Result<String, ProviderError> {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return Err(ProviderError::internal(
                "the local Claude sign-in listener stopped",
            ));
        };
        let mut stream = BufReader::new(stream);
        let mut request_line = String::new();
        if stream.read_line(&mut request_line).await.is_err() {
            continue;
        }

        match parse_callback_request(&request_line, state) {
            CallbackOutcome::Code(code) => {
                let _ = respond_html(
                    stream.into_inner(),
                    "EyeUrAI — Claude sign-in complete. You can close this tab and return to EyeUrAI.",
                )
                .await;
                return Ok(code);
            }
            CallbackOutcome::Denied(message) => {
                let _ = respond_html(
                    stream.into_inner(),
                    "EyeUrAI — Claude sign-in was not completed. You can close this tab.",
                )
                .await;
                return Err(ProviderError::unauthorized(message));
            }
            CallbackOutcome::NotTheCallback => {
                // Favicon probes, other-state requests, port scans: answer
                // and keep waiting for the real redirect.
                let _ = respond_html(
                    stream.into_inner(),
                    "EyeUrAI is waiting for the Claude sign-in redirect.",
                )
                .await;
            }
        }
    }
}

enum CallbackOutcome {
    Code(String),
    Denied(String),
    NotTheCallback,
}

/// Parse an HTTP request line like `GET /callback?code=...&state=... HTTP/1.1`.
/// Anything that is not the expected callback with the expected state is
/// ignored rather than trusted.
fn parse_callback_request(request_line: &str, expected_state: &str) -> CallbackOutcome {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" || !target.starts_with("/callback") {
        return CallbackOutcome::NotTheCallback;
    }
    let Ok(url) = url::Url::parse(&format!("http://localhost{target}")) else {
        return CallbackOutcome::NotTheCallback;
    };

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }

    if state.as_deref() != Some(expected_state) {
        return CallbackOutcome::NotTheCallback;
    }
    if let Some(error) = error {
        let message = if error == "access_denied" {
            "Claude sign-in was cancelled.".to_string()
        } else {
            "Claude sign-in was not completed.".to_string()
        };
        return CallbackOutcome::Denied(message);
    }
    match code.filter(|code| !code.is_empty()) {
        Some(code) => CallbackOutcome::Code(code),
        None => CallbackOutcome::Denied("Claude sign-in returned no code.".to_string()),
    }
}

async fn respond_html(mut stream: tokio::net::TcpStream, message: &str) -> std::io::Result<()> {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>EyeUrAI</title></head><body style=\"font-family:system-ui;margin:48px;color:#222\"><p>{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

// ---------------------------------------------------------------------------
// Grant persistence
// ---------------------------------------------------------------------------

struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_at_ms: i64,
    scope: Option<String>,
}

fn grant_from_token_response(
    response: &Value,
    now: DateTime<Utc>,
) -> Result<TokenResponse, ProviderError> {
    let access_token = response
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderError::parse("Claude sign-in returned no access token"))?;
    let refresh_token = response
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderError::parse("Claude sign-in returned no refresh token"))?;
    let expires_at_ms = response
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .map(|seconds| (now + chrono::Duration::seconds(seconds)).timestamp_millis())
        .unwrap_or(0);
    let scope = response
        .get("scope")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok(TokenResponse {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        expires_at_ms,
        scope,
    })
}

fn read_grant(profile_home: &Path) -> Result<StoredGrant, ProviderError> {
    let auth_path = profile_home.join(AUTH_FILE);
    let auth_is_regular_file = fs::symlink_metadata(&auth_path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false);
    if !auth_is_regular_file {
        return Err(ProviderError::credentials_missing(
            "this EyeUrAI Claude account is not signed in",
        )
        .with_remediation("Add the Claude account again in EyeUrAI settings."));
    }
    let blob = fs::read_to_string(&auth_path)
        .map_err(|_| ProviderError::internal("could not read the Claude account credential"))?;
    let grant: StoredGrant = serde_json::from_str(&blob).map_err(|_| {
        ProviderError::parse("the stored Claude account credential is not readable")
            .with_remediation("Add the Claude account again in EyeUrAI settings.")
    })?;
    if grant.access_token.is_empty() || grant.refresh_token.is_empty() {
        return Err(ProviderError::credentials_missing(
            "this EyeUrAI Claude account is not signed in",
        )
        .with_remediation("Add the Claude account again in EyeUrAI settings."));
    }
    Ok(grant)
}

/// Write the grant atomically: owner-only temporary file, fsync, replace.
/// Refresh rotation means a torn write here would lose the account, so the
/// replacement only happens after the bytes are durable. `NamedTempFile` uses
/// replace-existing semantics on Windows, where `std::fs::rename` cannot
/// overwrite the existing grant.
fn write_grant(profile_home: &Path, grant: &StoredGrant) -> Result<(), ProviderError> {
    let bytes = serde_json::to_vec_pretty(grant)
        .map_err(|_| ProviderError::internal("could not encode the Claude account credential"))?;
    let result = (|| -> std::io::Result<()> {
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(".{AUTH_FILE}.tmp-"))
            .tempfile_in(profile_home)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        let persisted = temporary
            .persist(profile_home.join(AUTH_FILE))
            .map_err(|error| error.error)?;
        persisted.sync_all()?;

        #[cfg(unix)]
        fs::File::open(profile_home)?.sync_all()?;

        Ok(())
    })();
    if result.is_err() {
        return Err(ProviderError::internal(
            "could not save the Claude account credential",
        ));
    }
    Ok(())
}

fn millis_to_utc(millis: i64) -> Option<DateTime<Utc>> {
    if millis <= 0 {
        return None;
    }
    Utc.timestamp_millis_opt(millis).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_store::{create_private_directory, unique_profile_id};
    use std::path::PathBuf;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            TestDirectory(
                std::env::temp_dir()
                    .join(format!("eyeurai-claude-profiles-{}", unique_profile_id())),
            )
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn base64_url_matches_rfc4648_test_vectors() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(b"foob"), "Zm9vYg");
        assert_eq!(base64_url(&[0xff, 0xef]), "_-8");
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_appendix_b() {
        // The RFC's worked example: verifier -> S256 challenge.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_carries_every_required_parameter() {
        let url = build_authorize_url("http://localhost:7777/callback", "challenge-x", "state-y");
        let parsed = url::Url::parse(&url).expect("valid URL");
        assert_eq!(parsed.host_str(), Some("claude.com"));
        assert_eq!(parsed.path(), "/cai/oauth/authorize");
        let pairs: std::collections::BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("code").map(String::as_str), Some("true"));
        assert_eq!(pairs.get("client_id").map(String::as_str), Some(CLIENT_ID));
        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("http://localhost:7777/callback")
        );
        assert_eq!(pairs.get("scope").map(String::as_str), Some("user:profile"));
        assert_eq!(
            pairs.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(pairs.get("state").map(String::as_str), Some("state-y"));
    }

    #[test]
    fn callback_parsing_requires_the_expected_state() {
        let ok = parse_callback_request("GET /callback?code=abc&state=xyz HTTP/1.1", "xyz");
        assert!(matches!(ok, CallbackOutcome::Code(code) if code == "abc"));

        let wrong_state =
            parse_callback_request("GET /callback?code=abc&state=nope HTTP/1.1", "xyz");
        assert!(matches!(wrong_state, CallbackOutcome::NotTheCallback));

        let favicon = parse_callback_request("GET /favicon.ico HTTP/1.1", "xyz");
        assert!(matches!(favicon, CallbackOutcome::NotTheCallback));

        let denied = parse_callback_request(
            "GET /callback?error=access_denied&state=xyz HTTP/1.1",
            "xyz",
        );
        assert!(matches!(denied, CallbackOutcome::Denied(_)));

        let no_code = parse_callback_request("GET /callback?state=xyz HTTP/1.1", "xyz");
        assert!(matches!(no_code, CallbackOutcome::Denied(_)));
    }

    #[test]
    fn token_response_parsing_requires_both_tokens() {
        let now = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
        let full = json!({
            "access_token": "FAKE-access",
            "refresh_token": "FAKE-refresh",
            "expires_in": 3600,
            "scope": "user:profile"
        });
        let parsed = grant_from_token_response(&full, now).expect("parses");
        assert_eq!(parsed.access_token, "FAKE-access");
        assert_eq!(parsed.refresh_token, "FAKE-refresh");
        assert_eq!(parsed.expires_at_ms, 1_700_003_600_000);
        assert_eq!(parsed.scope.as_deref(), Some("user:profile"));

        let missing = json!({"access_token": "FAKE-access"});
        assert!(grant_from_token_response(&missing, now).is_err());
    }

    #[test]
    fn grants_roundtrip_and_reject_empty_tokens() {
        let temp = TestDirectory::new();
        let root = store(&temp.0).root().to_path_buf();
        create_private_directory(&root).unwrap();
        let profile = root.join("profile-test");
        create_private_directory(&profile).unwrap();

        let grant = StoredGrant {
            version: 1,
            access_token: "FAKE-access".to_string(),
            refresh_token: "FAKE-refresh".to_string(),
            expires_at_ms: 1_700_003_600_000,
            scope: Some("user:profile".to_string()),
            email: Some("dev@example.com".to_string()),
            account_uuid: Some("uuid-1".to_string()),
            plan: Some("Max 20x".to_string()),
        };
        write_grant(&profile, &grant).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(profile.join(AUTH_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let read = read_grant(&profile).unwrap();
        assert_eq!(read.access_token, "FAKE-access");
        assert_eq!(read.refresh_token, "FAKE-refresh");
        assert_eq!(read.email.as_deref(), Some("dev@example.com"));

        let rotated = StoredGrant {
            access_token: "FAKE-rotated-access".to_string(),
            refresh_token: "FAKE-rotated-refresh".to_string(),
            ..grant
        };
        write_grant(&profile, &rotated).expect("an existing grant is atomically replaced");
        let read = read_grant(&profile).unwrap();
        assert_eq!(read.access_token, "FAKE-rotated-access");
        assert_eq!(read.refresh_token, "FAKE-rotated-refresh");

        fs::write(
            profile.join(AUTH_FILE),
            r#"{"version":1,"accessToken":"","refreshToken":""}"#,
        )
        .unwrap();
        let err = read_grant(&profile).expect_err("empty tokens are missing credentials");
        assert_eq!(
            err.kind,
            crate::models::ProviderErrorKind::CredentialsMissing
        );
    }

    #[test]
    fn discovers_only_completed_profiles() {
        let temp = TestDirectory::new();
        let root = store(&temp.0).root().to_path_buf();
        create_private_directory(&root).unwrap();

        let complete = root.join("profile-complete");
        create_private_directory(&complete).unwrap();
        fs::write(complete.join(AUTH_FILE), "{}").unwrap();
        let incomplete = root.join("profile-incomplete");
        create_private_directory(&incomplete).unwrap();

        let descriptors = discover_descriptors(&temp.0).unwrap();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, "claude-profile:profile-complete");
        assert_eq!(
            descriptors[0].option("claude_home"),
            Some(complete.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn requested_scope_stays_read_only() {
        // The stored token must never be able to run inference.
        assert_eq!(SCOPES, "user:profile");
    }
}
