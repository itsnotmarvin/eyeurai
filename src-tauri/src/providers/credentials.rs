//! Reading credentials *that already exist on this machine*.
//!
//! EyeUrAI is a read-only observer. It never writes to a vendor credential
//! store, never rotates a refresh token, and never copies a secret anywhere.
//! Concretely:
//!
//! * Claude is read from the credential `claude /login` already wrote — the
//!   macOS Keychain item `Claude Code-credentials`, or
//!   `~/.claude/.credentials.json` elsewhere.
//! * Codex is read from `~/.codex/auth.json`, written by `codex login`.
//! * OpenRouter / Gemini keys come from an environment variable or the system
//!   keychain — always named by an
//!   [`AccountDescriptor`], never embedded in one.
//!
//! Deliberately **not** implemented: OAuth refresh. Refreshing rotates the
//! stored refresh token; a monitor that refreshes without writing the rotated
//! token back would silently invalidate the user's `claude`/`codex` session.
//! An expired token therefore surfaces as an actionable `Unauthorized` with a
//! "run `claude /login`" remediation instead.
//!
//! The parsing functions in this module are pure and unit tested; the I/O
//! wrappers around them are thin by design.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::models::{AccountDescriptor, CredentialRef, ProviderId};

use super::error::ProviderError;
use super::jwt;
use super::secret::Secret;
use super::BoxFuture;

/// Non-secret identity hints scraped from a credential blob.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountHint {
    /// Stable upstream subject / account id, when the blob carries one.
    pub subject: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Extra non-secret metadata for diagnostics.
    pub extra: BTreeMap<String, String>,
}

/// The output of credential resolution. Holds [`Secret`]s, so it is not
/// serializable and must never be placed inside a snapshot.
#[derive(Debug)]
pub struct ResolvedCredential {
    pub access_token: Secret,
    /// Access-token expiry, when the store states it or the token is a JWT.
    pub expires_at: Option<DateTime<Utc>>,
    pub hint: AccountHint,
}

impl ResolvedCredential {
    pub fn bearer(token: Secret) -> Self {
        ResolvedCredential {
            access_token: token,
            expires_at: None,
            hint: AccountHint::default(),
        }
    }

    /// True when the token is known to have expired at `now`. Unknown expiry
    /// returns `false` — we would rather make a request that 401s than hide a
    /// working account because of clock skew.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.map(|at| at <= now).unwrap_or(false)
    }
}

/// Resolves an [`AccountDescriptor`] into credential material.
///
/// This is the *only* seam through which provider adapters touch secrets.
/// The main agent can supply another implementation without any adapter
/// changing.
pub trait CredentialResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        descriptor: &'a AccountDescriptor,
    ) -> BoxFuture<'a, Result<ResolvedCredential, ProviderError>>;
}

/// Timeout for reading a local credential (keychain prompts included).
/// Only referenced on macOS, where the keychain read shells out.
#[allow(dead_code)]
const CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Default resolver, reading only from stores that already exist on disk.
#[derive(Debug, Default, Clone)]
pub struct LocalCredentialResolver {
    /// Overrides `$HOME` resolution. Test seam only.
    home_override: Option<PathBuf>,
}

impl LocalCredentialResolver {
    pub fn new() -> Self {
        LocalCredentialResolver::default()
    }

    fn home(&self) -> Result<PathBuf, ProviderError> {
        if let Some(home) = &self.home_override {
            return Ok(home.clone());
        }
        dirs::home_dir().ok_or_else(|| {
            ProviderError::credentials_missing("could not determine your home directory")
        })
    }

    async fn resolve_inner(
        &self,
        descriptor: &AccountDescriptor,
    ) -> Result<ResolvedCredential, ProviderError> {
        match &descriptor.credential {
            CredentialRef::ClaudeCli => self.read_claude().await,
            // EyeUrAI-owned Claude profiles refresh and rotate their own
            // grant, which this read-only resolver must never do; the Claude
            // adapter resolves them through `claude_profiles` instead.
            CredentialRef::ClaudeProfile { .. } => Err(ProviderError::internal(
                "EyeUrAI Claude accounts are resolved by their profile manager",
            )),
            CredentialRef::CodexCli { path } => self.read_codex(path.as_deref()).await,
            CredentialRef::Env { var } => read_env(var),
            CredentialRef::Keychain { service, account } => {
                read_keychain(service, account.as_deref()).await
            }
            CredentialRef::None => Err(ProviderError::credentials_missing(
                "this account has no credential configured",
            )),
        }
    }

    async fn read_claude(&self) -> Result<ResolvedCredential, ProviderError> {
        #[cfg(target_os = "macos")]
        {
            match read_keychain_raw("Claude Code-credentials", None).await {
                Ok(blob) => return parse_claude_credentials(&blob),
                Err(err) if err.kind == crate::models::ProviderErrorKind::CredentialsMissing => {
                    // Fall through to the file location: some setups (or a
                    // Claude Code run under a different keychain) use it.
                }
                Err(err) => return Err(err),
            }
        }

        let path = self.home()?.join(".claude").join(".credentials.json");
        let blob = read_file(&path).await.map_err(|err| {
            if err.kind == crate::models::ProviderErrorKind::CredentialsMissing {
                ProviderError::credentials_missing("no Claude Code login was found")
                    .with_remediation("Run `claude /login` in a terminal, then refresh.")
            } else {
                err
            }
        })?;
        parse_claude_credentials(&blob)
    }

    async fn read_codex(&self, path: Option<&str>) -> Result<ResolvedCredential, ProviderError> {
        let path = match path {
            Some(p) => PathBuf::from(p),
            None => self.home()?.join(".codex").join("auth.json"),
        };
        let blob = read_file(&path).await.map_err(|err| {
            if err.kind == crate::models::ProviderErrorKind::CredentialsMissing {
                ProviderError::credentials_missing("no Codex login was found at ~/.codex/auth.json")
                    .with_remediation("Run `codex login` in a terminal, then refresh.")
            } else {
                err
            }
        })?;
        parse_codex_auth(&blob)
    }
}

impl CredentialResolver for LocalCredentialResolver {
    fn resolve<'a>(
        &'a self,
        descriptor: &'a AccountDescriptor,
    ) -> BoxFuture<'a, Result<ResolvedCredential, ProviderError>> {
        Box::pin(async move { self.resolve_inner(descriptor).await })
    }
}

// ---------------------------------------------------------------------------
// Raw readers
// ---------------------------------------------------------------------------

fn read_env(var: &str) -> Result<ResolvedCredential, ProviderError> {
    match std::env::var(var) {
        Ok(value) if !value.trim().is_empty() => {
            Ok(ResolvedCredential::bearer(Secret::new(value.trim())))
        }
        _ => Err(ProviderError::credentials_missing(format!(
            "environment variable {var} is not set"
        ))
        .with_remediation(format!("Set {var}, or add the key in EyeUrAI settings."))),
    }
}

async fn read_file(path: &std::path::Path) -> Result<String, ProviderError> {
    let path = path.to_path_buf();
    let display = path.display().to_string();
    let result = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
        .await
        .map_err(|_| ProviderError::internal("credential read task failed"))?;

    result.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            ProviderError::credentials_missing(format!("{display} does not exist"))
        }
        std::io::ErrorKind::PermissionDenied => {
            ProviderError::credentials_missing(format!("{display} is not readable"))
                .with_remediation("Check the file's permissions.")
        }
        _ => ProviderError::internal(format!("could not read {display}")),
    })
}

#[cfg(target_os = "macos")]
async fn read_keychain_raw(service: &str, account: Option<&str>) -> Result<String, ProviderError> {
    let mut command = tokio::process::Command::new("security");
    command.arg("find-generic-password").arg("-s").arg(service);
    if let Some(account) = account {
        command.arg("-a").arg(account);
    }
    command.arg("-w");

    let output = tokio::time::timeout(CREDENTIAL_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            ProviderError::timeout("reading the system keychain timed out").with_remediation(
                "Approve the keychain prompt, or grant EyeUrAI access in Keychain Access.",
            )
        })?
        .map_err(|_| ProviderError::internal("could not run the macOS `security` tool"))?;

    if !output.status.success() {
        // `security` exits 44 (errSecItemNotFound) when the item is absent.
        if output.status.code() == Some(44) {
            return Err(ProviderError::credentials_missing(format!(
                "keychain item \"{service}\" was not found"
            )));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("could not be found") {
            return Err(ProviderError::credentials_missing(format!(
                "keychain item \"{service}\" was not found"
            )));
        }
        return Err(ProviderError::credentials_missing(format!(
            "the system keychain denied access to \"{service}\""
        ))
        .with_remediation("Approve the keychain prompt when macOS asks, then refresh."));
    }

    // `security -w` appends exactly one newline.
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.ends_with('\n') {
        text.pop();
    }
    Ok(text)
}

#[cfg(not(target_os = "macos"))]
async fn read_keychain_raw(service: &str, _account: Option<&str>) -> Result<String, ProviderError> {
    Err(ProviderError::unsupported(format!(
        "reading keychain item \"{service}\" is only supported on macOS"
    )))
}

async fn read_keychain(
    service: &str,
    account: Option<&str>,
) -> Result<ResolvedCredential, ProviderError> {
    let raw = read_keychain_raw(service, account).await?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::credentials_missing(format!(
            "keychain item \"{service}\" is empty"
        )));
    }
    Ok(ResolvedCredential::bearer(Secret::new(trimmed)))
}

// ---------------------------------------------------------------------------
// Pure parsers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ClaudeCredentialFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOauthBlock>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOauthBlock {
    #[serde(rename = "accessToken", default)]
    access_token: Option<String>,
    /// Milliseconds since the Unix epoch.
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<i64>,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: Option<String>,
    /// Present on some Claude Code versions.
    #[serde(rename = "scopes", default)]
    scopes: Option<Vec<String>>,
}

/// Parse the JSON blob `claude /login` stores (identical on macOS Keychain and
/// on the Linux/Windows credentials file).
///
/// Only the access token is required. The refresh token is intentionally
/// **not** read: this app never refreshes.
pub fn parse_claude_credentials(blob: &str) -> Result<ResolvedCredential, ProviderError> {
    let parsed: ClaudeCredentialFile = serde_json::from_str(blob).map_err(|_| {
        ProviderError::parse("the stored Claude credential is not valid JSON")
            .with_remediation("Run `claude /login` to rewrite it.")
    })?;

    let block = parsed.claude_ai_oauth.ok_or_else(|| {
        ProviderError::credentials_missing("the stored Claude credential has no OAuth section")
            .with_remediation("Run `claude /login` in a terminal, then refresh.")
    })?;

    let token = block
        .access_token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            ProviderError::credentials_missing("the stored Claude credential has no access token")
                .with_remediation("Run `claude /login` in a terminal, then refresh.")
        })?;

    let expires_at = block
        .expires_at
        .filter(|ms| *ms > 0)
        .and_then(millis_to_utc);

    let mut hint = AccountHint {
        plan: block.subscription_type.filter(|s| !s.is_empty()),
        ..AccountHint::default()
    };
    if let Some(scopes) = block.scopes {
        if !scopes.is_empty() {
            hint.extra.insert("scopes".to_string(), scopes.join(" "));
        }
    }

    Ok(ResolvedCredential {
        access_token: Secret::new(token.trim()),
        expires_at,
        hint,
    })
}

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    #[serde(default)]
    tokens: Option<CodexTokens>,
    /// Present when the user is on an API-key setup rather than a ChatGPT
    /// subscription. We do not use it for quota (there is no quota endpoint
    /// for API keys) but its presence changes the remediation text.
    #[serde(rename = "OPENAI_API_KEY", default)]
    openai_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

/// Parse `~/.codex/auth.json`.
///
/// Identity (`sub`, `email`, plan) is read from the `id_token` claims when
/// present; expiry is read from the access token's `exp` claim. The refresh
/// token is intentionally not read.
pub fn parse_codex_auth(blob: &str) -> Result<ResolvedCredential, ProviderError> {
    let parsed: CodexAuthFile = serde_json::from_str(blob).map_err(|_| {
        ProviderError::parse("~/.codex/auth.json is not valid JSON")
            .with_remediation("Run `codex login` to rewrite it.")
    })?;

    let tokens = parsed.tokens.unwrap_or(CodexTokens {
        access_token: None,
        id_token: None,
        account_id: None,
    });

    let access = tokens
        .access_token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            if parsed.openai_api_key.is_some() {
                ProviderError::unsupported(
                    "this Codex install uses an API key; ChatGPT quota windows are only reported for subscription logins",
                )
                .with_remediation("Run `codex login` with your ChatGPT account to see quota.")
            } else {
                ProviderError::credentials_missing("~/.codex/auth.json has no access token")
                    .with_remediation("Run `codex login` in a terminal, then refresh.")
            }
        })?;

    let access = Secret::new(access.trim());
    let id_token = tokens
        .id_token
        .filter(|t| !t.trim().is_empty())
        .map(|t| Secret::new(t.trim()));

    let stored_account_id = tokens
        .account_id
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string());

    let mut hint = AccountHint::default();
    if let Some(id_token) = &id_token {
        if let Ok(claims) = jwt::read_claims(id_token) {
            hint.subject = claims.subject;
            hint.email = claims.email;
            hint.plan = claims.chatgpt_plan_type;
            if let Some(account_id) = claims.chatgpt_account_id {
                hint.extra
                    .insert("chatgpt_account_id".to_string(), account_id);
            }
        }
    }
    if let Some(account_id) = &stored_account_id {
        hint.extra
            .insert("codex_account_id".to_string(), account_id.clone());
    }
    if hint.subject.is_none() {
        hint.subject = stored_account_id;
    }

    let expires_at = jwt::expiry_seconds(&access).and_then(seconds_to_utc);

    Ok(ResolvedCredential {
        access_token: access,
        expires_at,
        hint,
    })
}

fn millis_to_utc(millis: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(millis).single()
}

fn seconds_to_utc(seconds: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(seconds, 0).single()
}

// ---------------------------------------------------------------------------
// Descriptor discovery
// ---------------------------------------------------------------------------

/// Environment variable names checked for each key-based provider, in order.
pub const OPENROUTER_ENV_VARS: [&str; 2] = ["OPENROUTER_API_KEY", "OPENROUTER_KEY"];
pub const GEMINI_TOKEN_ENV_VARS: [&str; 2] = ["GOOGLE_CLOUD_ACCESS_TOKEN", "GEMINI_ACCESS_TOKEN"];
pub const GEMINI_PROJECT_ENV_VARS: [&str; 3] = [
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GCLOUD_PROJECT",
];

/// Descriptors EyeUrAI proposes on first run, with no I/O beyond an
/// environment lookup. Nothing here reads a secret's *value*: the OpenRouter
/// and Gemini entries only record which variable to read later.
pub fn default_descriptors() -> Vec<AccountDescriptor> {
    let mut out = vec![
        AccountDescriptor::new(ProviderId::Claude, "claude-cli", CredentialRef::ClaudeCli)
            .with_label("Claude Code login"),
        AccountDescriptor::new(
            ProviderId::Codex,
            "codex-cli",
            CredentialRef::CodexCli { path: None },
        )
        .with_label("Codex CLI login"),
    ];

    if let Some(var) = first_set_env(&OPENROUTER_ENV_VARS) {
        out.push(
            AccountDescriptor::new(
                ProviderId::OpenRouter,
                "openrouter-env",
                CredentialRef::Env { var: var.clone() },
            )
            .with_label(format!("OpenRouter key (${var})")),
        );
    }

    if let (Some(token_var), Some(project)) = (
        first_set_env(&GEMINI_TOKEN_ENV_VARS),
        first_set_env_value(&GEMINI_PROJECT_ENV_VARS),
    ) {
        out.push(
            AccountDescriptor::new(
                ProviderId::Gemini,
                "gemini-cloud-quotas",
                CredentialRef::Env { var: token_var },
            )
            .with_label("Gemini (Cloud Quotas)")
            .with_option("project_id", project),
        );
    }

    out
}

/// Name of the first variable in `candidates` that is set and non-empty.
fn first_set_env(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|name| {
            std::env::var(name)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
        })
        .map(|name| (*name).to_string())
}

/// Value of the first variable in `candidates` that is set. Only used for
/// non-secret values such as a project id.
fn first_set_env_value(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderErrorKind;

    // NOTE: every token below is fabricated. `FAKE-` prefixes and the literal
    // "not-a-real-signature" keep these fixtures obviously non-secret.

    fn fake_jwt(payload: &str) -> String {
        fn b64url(bytes: &[u8]) -> String {
            const ALPHABET: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let b0 = chunk[0] as u32;
                let b1 = *chunk.get(1).unwrap_or(&0) as u32;
                let b2 = *chunk.get(2).unwrap_or(&0) as u32;
                let n = (b0 << 16) | (b1 << 8) | b2;
                let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
                let take = match chunk.len() {
                    1 => 2,
                    2 => 3,
                    _ => 4,
                };
                for i in idx.iter().take(take) {
                    out.push(ALPHABET[*i as usize] as char);
                }
            }
            out
        }
        format!(
            "{}.{}.{}",
            b64url(br#"{"alg":"none"}"#),
            b64url(payload.as_bytes()),
            b64url(b"not-a-real-signature")
        )
    }

    #[test]
    fn parses_a_claude_credential_blob() {
        let blob = r#"{
            "claudeAiOauth": {
                "accessToken": "FAKE-claude-access-token",
                "refreshToken": "FAKE-claude-refresh-token",
                "expiresAt": 1893456000000,
                "subscriptionType": "max",
                "scopes": ["user:inference", "user:profile"]
            }
        }"#;
        let cred = parse_claude_credentials(blob).expect("parses");
        assert_eq!(cred.access_token.expose(), "FAKE-claude-access-token");
        assert_eq!(cred.hint.plan.as_deref(), Some("max"));
        assert_eq!(
            cred.expires_at,
            Some(Utc.timestamp_opt(1_893_456_000, 0).single().unwrap())
        );
        assert_eq!(
            cred.hint.extra.get("scopes").map(String::as_str),
            Some("user:inference user:profile")
        );
        // The refresh token is deliberately never read.
        assert!(format!("{cred:?}").contains("<redacted>"));
    }

    #[test]
    fn claude_blob_without_a_token_is_credentials_missing() {
        let err = parse_claude_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#)
            .expect_err("no token");
        assert_eq!(err.kind, ProviderErrorKind::CredentialsMissing);
        assert!(err
            .remediation
            .as_deref()
            .unwrap()
            .contains("claude /login"));

        let err = parse_claude_credentials(r#"{"other":true}"#).expect_err("no oauth block");
        assert_eq!(err.kind, ProviderErrorKind::CredentialsMissing);
    }

    #[test]
    fn claude_blob_that_is_not_json_is_a_parse_error() {
        let err = parse_claude_credentials("not json").expect_err("bad json");
        assert_eq!(err.kind, ProviderErrorKind::Parse);
    }

    #[test]
    fn claude_blob_tolerates_unknown_future_fields() {
        let blob = r#"{
            "claudeAiOauth": {"accessToken":"FAKE-token","someNewField":{"a":1}},
            "anotherTopLevelThing": [1,2,3]
        }"#;
        let cred = parse_claude_credentials(blob).expect("parses");
        assert_eq!(cred.access_token.expose(), "FAKE-token");
        assert_eq!(cred.expires_at, None);
    }

    #[test]
    fn parses_a_codex_auth_file() {
        let id_token = fake_jwt(
            r#"{"sub":"user-123","email":"dev@example.com",
                "https://api.openai.com/auth":{"chatgpt_plan_type":"pro","chatgpt_account_id":"acct_1"}}"#,
        );
        let access_token = fake_jwt(r#"{"exp":1893456000}"#);
        let blob = format!(
            r#"{{"tokens":{{"access_token":"{access_token}","refresh_token":"FAKE-refresh","id_token":"{id_token}","account_id":"acct_1"}},"last_refresh":"2026-01-01T00:00:00Z"}}"#
        );

        let cred = parse_codex_auth(&blob).expect("parses");
        assert_eq!(cred.access_token.expose(), access_token);
        assert_eq!(cred.hint.subject.as_deref(), Some("user-123"));
        assert_eq!(cred.hint.email.as_deref(), Some("dev@example.com"));
        assert_eq!(cred.hint.plan.as_deref(), Some("pro"));
        assert_eq!(
            cred.hint
                .extra
                .get("chatgpt_account_id")
                .map(String::as_str),
            Some("acct_1")
        );
        assert_eq!(
            cred.hint.extra.get("codex_account_id").map(String::as_str),
            Some("acct_1")
        );
        assert_eq!(
            cred.expires_at,
            Some(Utc.timestamp_opt(1_893_456_000, 0).single().unwrap())
        );
    }

    #[test]
    fn codex_falls_back_to_account_id_when_id_token_is_opaque() {
        let blob = r#"{"tokens":{"access_token":"FAKE-opaque-access","id_token":"opaque","account_id":"acct_fallback"}}"#;
        let cred = parse_codex_auth(blob).expect("parses");
        assert_eq!(cred.hint.subject.as_deref(), Some("acct_fallback"));
        assert_eq!(cred.expires_at, None);
    }

    #[test]
    fn codex_api_key_only_install_reports_unsupported() {
        let blob = r#"{"OPENAI_API_KEY":"FAKE-not-a-real-key","tokens":{}}"#;
        let err = parse_codex_auth(blob).expect_err("no subscription token");
        assert_eq!(err.kind, ProviderErrorKind::Unsupported);
        assert!(err.remediation.as_deref().unwrap().contains("codex login"));
    }

    #[test]
    fn codex_missing_tokens_is_credentials_missing() {
        let err = parse_codex_auth(r#"{"tokens":{}}"#).expect_err("no token");
        assert_eq!(err.kind, ProviderErrorKind::CredentialsMissing);
        let err = parse_codex_auth(r#"{}"#).expect_err("no tokens block");
        assert_eq!(err.kind, ProviderErrorKind::CredentialsMissing);
    }

    #[test]
    fn expiry_check_is_conservative_when_unknown() {
        let now = Utc.timestamp_opt(1_000_000, 0).single().unwrap();
        let mut cred = ResolvedCredential::bearer(Secret::new("FAKE"));
        assert!(!cred.is_expired(now));
        cred.expires_at = Some(Utc.timestamp_opt(999_999, 0).single().unwrap());
        assert!(cred.is_expired(now));
        cred.expires_at = Some(Utc.timestamp_opt(1_000_001, 0).single().unwrap());
        assert!(!cred.is_expired(now));
    }

    #[test]
    fn default_descriptors_always_include_the_cli_backed_providers() {
        let descriptors = default_descriptors();
        assert!(descriptors
            .iter()
            .any(|d| d.provider == ProviderId::Claude && d.credential == CredentialRef::ClaudeCli));
        assert!(descriptors.iter().any(|d| d.provider == ProviderId::Codex));
        // Descriptors are pure references; no descriptor may embed a value.
        let json = serde_json::to_string(&descriptors).expect("serialize");
        assert!(!json.contains("sk-"));
    }
}
