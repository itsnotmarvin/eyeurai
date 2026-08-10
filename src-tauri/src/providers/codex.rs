//! OpenAI Codex / ChatGPT subscription quota windows.
//!
//! # Data source
//!
//! The default terminal login is read through the first-party usage route with
//! the OAuth access token stored in `~/.codex/auth.json`. Accounts added in
//! EyeUrAI are instead read through the official Codex app-server's
//! `account/read` and `account/rateLimits/read` methods, one isolated
//! `CODEX_HOME` per account.
//!
//! # Window naming
//!
//! The response returns `primary_window` / `secondary_window` *slots*, not
//! named durations. Naming a window after its slot is wrong: on plans without
//! a 5-hour tier the weekly cap arrives in `primary_window`. We therefore key
//! each window off `limit_window_seconds` (±5% tolerance) and fall back to
//! `window_<n>s` for durations we have not seen, so new tiers still surface
//! rather than disappearing.
//!
//! # Identity
//!
//! Account identity comes from the `id_token`'s `sub`/`email` claims and the
//! `https://api.openai.com/auth` claim block (plan type). Signatures are not
//! verified — see [`super::jwt`] for why that is correct here.
//!
//! EyeUrAI never refreshes the default CLI token. For isolated profiles, the
//! Codex app-server owns persistence and token rotation inside that profile.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::models::{
    AccountDescriptor, AccountSnapshot, CapabilityLevel, CredentialKind, Freshness,
    ProviderCapability, ProviderErrorKind, ProviderId, ProviderSnapshot, QuotaWindow, WindowKind,
};

use super::credentials::AccountHint;
use super::error::ProviderError;
use super::http::JsonRequest;
use super::{not_configured, BoxFuture, ProviderContext, QuotaProvider};
use crate::codex_profiles;

const USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const USAGE_TIMEOUT: Duration = Duration::from_secs(10);

const LOGIN_HINT: &str = "Run `codex login` in a terminal, then refresh.";
const REVOKED_HINT: &str =
    "This login was revoked, usually because another account signed in. Run `codex login` again.";

/// `(nominal seconds, key, label)` for durations we recognise.
const KNOWN_WINDOWS: [(i64, &str, &str); 4] = [
    (18_000, "five_hour", "5-hour session"),
    (86_400, "one_day", "Daily"),
    (604_800, "seven_day", "Weekly"),
    (2_592_000, "thirty_day", "Monthly"),
];

pub struct CodexProvider;

impl CodexProvider {
    pub fn new() -> Self {
        CodexProvider
    }

    pub fn capability_info() -> ProviderCapability {
        ProviderCapability {
            provider: ProviderId::Codex,
            display_name: ProviderId::Codex.display_name().to_string(),
            level: CapabilityLevel::Full,
            data_source: "Existing Codex CLI login plus official Codex app-server profiles"
                .to_string(),
            official_api: false,
            read_only: true,
            supports_multiple_accounts: true,
            supports_percent: true,
            supports_reset_times: true,
            supports_currency: false,
            credential_kinds: vec![CredentialKind::CodexCli],
            option_keys: vec![],
            notes: vec![
                "Requires a ChatGPT subscription login; API-key-only Codex installs have no quota endpoint."
                    .to_string(),
                "This is a first-party endpoint, not a documented public API, so the reported windows can change."
                    .to_string(),
                "EyeUrAI never refreshes or rewrites the default ~/.codex/auth.json login."
                    .to_string(),
                "Accounts added in EyeUrAI use isolated CODEX_HOME directories managed by the official Codex app-server."
                    .to_string(),
            ],
            doc_url: None,
        }
    }
}

impl Default for CodexProvider {
    fn default() -> Self {
        CodexProvider::new()
    }
}

impl QuotaProvider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn capability(&self) -> ProviderCapability {
        CodexProvider::capability_info()
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a ProviderContext,
        accounts: &'a [AccountDescriptor],
    ) -> BoxFuture<'a, ProviderSnapshot> {
        Box::pin(async move {
            if accounts.is_empty() {
                return not_configured(CodexProvider::capability_info(), LOGIN_HINT);
            }

            let mut results = Vec::with_capacity(accounts.len());
            for (index, descriptor) in accounts.iter().enumerate() {
                results.push(fetch_one(ctx, descriptor, index == 0).await);
            }
            // A user may add the same account that their default terminal
            // happens to use. Prefer the independently managed profile in
            // that case, while retaining every distinct managed profile.
            let managed_labels: BTreeSet<String> = results
                .iter()
                .filter(|account| account.account_id.starts_with("codex-profile:"))
                .map(|account| account.label.to_ascii_lowercase())
                .collect();
            results.retain(|account| {
                account.account_id.starts_with("codex-profile:")
                    || !managed_labels.contains(&account.label.to_ascii_lowercase())
            });
            let mut observed = BTreeSet::new();
            results.retain(|account| observed.insert(account.account_id.clone()));

            let mut snapshot =
                ProviderSnapshot::new(CodexProvider::capability_info()).with_accounts(results);
            snapshot.freshness = Freshness::live(ctx.now());
            snapshot.sort_accounts();
            snapshot
        })
    }
}

async fn fetch_one(
    ctx: &ProviderContext,
    descriptor: &AccountDescriptor,
    is_primary: bool,
) -> AccountSnapshot {
    if let Some(profile_home) = descriptor.option("codex_home") {
        return fetch_managed_profile(ctx, descriptor, Path::new(profile_home)).await;
    }

    let now = ctx.now();
    let fallback_label = descriptor.fallback_label();

    let credential = match ctx.credentials.resolve(descriptor).await {
        Ok(credential) => credential,
        Err(err) => {
            let err = if err.kind == ProviderErrorKind::Unsupported {
                err
            } else {
                err.with_remediation(LOGIN_HINT)
            };
            let mut snapshot =
                AccountSnapshot::failed(descriptor.id.clone(), fallback_label, err.to_info());
            snapshot.active = is_primary;
            return snapshot;
        }
    };

    let account_id = stable_account_id(descriptor, &credential.hint);

    let label = credential
        .hint
        .email
        .clone()
        .or_else(|| descriptor.label.clone())
        .unwrap_or(fallback_label);
    let plan = credential.hint.plan.clone().map(|p| pretty_plan(&p));

    if credential.is_expired(now) {
        let mut snapshot = AccountSnapshot::failed(
            account_id.clone(),
            label,
            ProviderError::unauthorized("your Codex login has expired")
                .with_remediation(LOGIN_HINT)
                .to_info(),
        );
        snapshot.plan = plan;
        snapshot.active = is_primary;
        return snapshot;
    }

    let response = ctx
        .http
        .get_json::<Value>(
            &JsonRequest::new(USAGE_ENDPOINT, ctx.user_agent(ProviderId::Codex))
                .bearer(&credential.access_token)
                .timeout(USAGE_TIMEOUT),
        )
        .await;

    let mut snapshot = match response {
        Ok(value) => match parse_usage(&value, now) {
            Ok(windows) => {
                let mut snapshot = AccountSnapshot::new(account_id.clone(), label);
                snapshot.windows = windows;
                snapshot.freshness = Freshness::live(now);
                snapshot
            }
            Err(err) => AccountSnapshot::failed(account_id.clone(), label, err.to_info()),
        },
        Err(err) => AccountSnapshot::failed(account_id, label, annotate(err).to_info()),
    };

    snapshot.plan = plan;
    snapshot.active = is_primary;
    snapshot
}

async fn fetch_managed_profile(
    ctx: &ProviderContext,
    descriptor: &AccountDescriptor,
    profile_home: &Path,
) -> AccountSnapshot {
    let now = ctx.now();
    let managed = match codex_profiles::read_managed_account(profile_home).await {
        Ok(account) => account,
        Err(error) => {
            let mut snapshot = AccountSnapshot::failed(
                descriptor.id.clone(),
                descriptor.fallback_label(),
                error.to_info(),
            );
            snapshot.active = true;
            return snapshot;
        }
    };

    // The isolated profile is the durable connection identity. Using an
    // email-derived ID here made the same connection change IDs whenever an
    // app-server read failed (the failure path only knows the descriptor),
    // which produced duplicate stale/error rows and broke disconnect filters.
    let account_id = descriptor.id.clone();
    let label = managed
        .email
        .clone()
        .unwrap_or_else(|| descriptor.fallback_label());
    let mut snapshot = match parse_app_server_rate_limits(&managed.rate_limits, now) {
        Ok(windows) => {
            let mut snapshot = AccountSnapshot::new(account_id.clone(), label.clone());
            snapshot.windows = windows;
            snapshot.freshness = Freshness::live(now);
            snapshot
        }
        Err(error) => AccountSnapshot::failed(account_id, label, error.to_info()),
    };
    snapshot.plan = managed.plan.map(|plan| pretty_plan(&plan));
    // Isolated profiles remain independently readable even when another Codex
    // account is added, so they are active rather than historical cache rows.
    snapshot.active = true;
    snapshot
}

/// Prefer the account identifiers Codex stores explicitly, then fall back to
/// the OIDC subject. All are non-secret identity claims; credential slot names
/// are used only when an older/opaque login exposes no principal at all.
fn stable_account_id(descriptor: &AccountDescriptor, hint: &AccountHint) -> String {
    hint.extra
        .get("chatgpt_account_id")
        .or_else(|| hint.extra.get("codex_account_id"))
        .or(hint.subject.as_ref())
        .and_then(|identity| prefixed_identity("codex:", identity, false))
        .unwrap_or_else(|| descriptor.id.clone())
}

fn prefixed_identity(prefix: &str, identity: &str, lowercase: bool) -> Option<String> {
    let identity = identity.trim();
    if identity.is_empty()
        || prefix.len() + identity.len() > 160
        || identity
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return None;
    }
    let normalized = if lowercase {
        identity.to_ascii_lowercase()
    } else {
        identity.to_string()
    };
    let fingerprint = Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(format!("{prefix}{fingerprint}"))
}

/// Attach the right remediation to an upstream failure.
///
/// OpenAI returns 401 with `token_revoked` / `token_invalidated` when a token
/// has been superseded by another `codex login` on the same machine. That is a
/// materially different situation from "your session expired", so it gets its
/// own wording.
pub fn annotate(err: ProviderError) -> ProviderError {
    if err.kind != ProviderErrorKind::Unauthorized {
        return err;
    }
    let lowered = err.message.to_ascii_lowercase();
    if lowered.contains("token_revoked") || lowered.contains("token_invalidated") {
        return ProviderError::unauthorized("this Codex login was revoked upstream")
            .with_remediation(REVOKED_HINT);
    }
    err.with_remediation(LOGIN_HINT)
}

/// `plus` -> `Plus`, `pro` -> `Pro`, `chatgpt_team` -> `Chatgpt Team`.
fn pretty_plan(raw: &str) -> String {
    raw.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse the `wham/usage` response into normalized windows.
pub fn parse_usage(value: &Value, now: DateTime<Utc>) -> Result<Vec<QuotaWindow>, ProviderError> {
    let object = value
        .as_object()
        .ok_or_else(|| ProviderError::parse("the Codex usage response was not a JSON object"))?;

    let rate_limit = object.get("rate_limit").and_then(Value::as_object);

    let mut windows = Vec::new();

    if let Some(rate_limit) = rate_limit {
        for slot in ["primary_window", "secondary_window"] {
            if let Some(entry) = rate_limit.get(slot).and_then(Value::as_object) {
                if let Some(window) = window_from(entry, now, "") {
                    push_unique(&mut windows, window);
                }
            }
        }
    }

    if let Some(entry) = object
        .get("code_review_rate_limit")
        .and_then(Value::as_object)
    {
        if let Some(window) = window_from(entry, now, "code_review_") {
            push_unique(&mut windows, window);
        }
    }

    if windows.is_empty() {
        if rate_limit.is_none() {
            return Err(ProviderError::parse(
                "the Codex usage response had no rate_limit section; the upstream API may have changed",
            )
            .with_remediation("Update EyeUrAI, or report this if it persists."));
        }
        // A brand-new account with no usage yet legitimately returns empty
        // windows. That is not an error — it is "nothing to show".
        return Ok(windows);
    }

    Ok(windows)
}

/// Parse the supported `account/rateLimits/read` app-server shape used for
/// EyeUrAI-managed Codex profiles.
pub fn parse_app_server_rate_limits(
    value: &Value,
    now: DateTime<Utc>,
) -> Result<Vec<QuotaWindow>, ProviderError> {
    let object = value.as_object().ok_or_else(|| {
        ProviderError::parse("the Codex app-server rate limits were not an object")
    })?;
    let mut windows = Vec::new();
    for slot in ["primary", "secondary"] {
        let Some(entry) = object.get(slot).and_then(Value::as_object) else {
            continue;
        };
        let Some(used_percent) = entry.get("usedPercent").and_then(Value::as_f64) else {
            continue;
        };
        let window_seconds = entry
            .get("windowDurationMins")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .saturating_mul(60);
        let reset_at = entry.get("resetsAt").and_then(Value::as_i64).unwrap_or(0);
        let mut normalized = serde_json::Map::new();
        normalized.insert("used_percent".to_string(), Value::from(used_percent));
        normalized.insert(
            "limit_window_seconds".to_string(),
            Value::from(window_seconds),
        );
        normalized.insert("reset_at".to_string(), Value::from(reset_at));
        if let Some(window) = window_from(&normalized, now, "") {
            push_unique(&mut windows, window);
        }
    }
    Ok(windows)
}

/// Later duplicates lose: `primary_window` wins over `secondary_window` when
/// both report the same duration.
fn push_unique(windows: &mut Vec<QuotaWindow>, window: QuotaWindow) {
    if windows.iter().any(|w| w.key == window.key) {
        return;
    }
    windows.push(window);
}

/// Convert one window object. Returns `None` for the all-zero placeholder
/// objects the endpoint emits for features the account has never used —
/// rendering those as "0% used, resets 1970-01-01" would be worse than
/// omitting them.
fn window_from(
    entry: &serde_json::Map<String, Value>,
    now: DateTime<Utc>,
    key_prefix: &str,
) -> Option<QuotaWindow> {
    let used_percent = entry.get("used_percent").and_then(Value::as_f64)?;
    let limit_window_seconds = entry
        .get("limit_window_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let reset_at = entry.get("reset_at").and_then(Value::as_i64).unwrap_or(0);

    if reset_at <= 0 && limit_window_seconds <= 0 && used_percent == 0.0 {
        return None;
    }

    let (key_suffix, label) = window_naming(limit_window_seconds);
    let key = format!("{key_prefix}{key_suffix}");
    let label = if key_prefix.is_empty() {
        label
    } else {
        format!("Code review · {label}")
    };

    let mut window =
        QuotaWindow::new(key, label, WindowKind::Rolling).with_used_percent(used_percent);
    if limit_window_seconds > 0 {
        window = window.with_window_seconds(limit_window_seconds);
    }
    if reset_at > 0 {
        if let Some(resets_at) = Utc.timestamp_opt(reset_at, 0).single() {
            window = window.with_reset(resets_at, now);
        }
    } else if let Some(reset_after) = entry.get("reset_after_seconds").and_then(Value::as_i64) {
        // Some responses only carry a relative countdown.
        if reset_after > 0 {
            window = window.with_reset(now + chrono::Duration::seconds(reset_after), now);
        }
    }

    Some(window)
}

/// Map a window duration onto a stable key and a human label, with a ±5%
/// tolerance because upstream values drift slightly.
pub fn window_naming(limit_window_seconds: i64) -> (String, String) {
    for (center, key, label) in KNOWN_WINDOWS {
        let tolerance = center / 20;
        if (center - tolerance..=center + tolerance).contains(&limit_window_seconds) {
            return (key.to_string(), label.to_string());
        }
    }
    if limit_window_seconds <= 0 {
        return ("window".to_string(), "Usage".to_string());
    }
    (
        format!("window_{limit_window_seconds}s"),
        humanize_duration(limit_window_seconds),
    )
}

fn humanize_duration(seconds: i64) -> String {
    if seconds % 86_400 == 0 {
        let days = seconds / 86_400;
        return if days == 1 {
            "1-day window".to_string()
        } else {
            format!("{days}-day window")
        };
    }
    if seconds % 3_600 == 0 {
        let hours = seconds / 3_600;
        return if hours == 1 {
            "1-hour window".to_string()
        } else {
            format!("{hours}-hour window")
        };
    }
    format!("{seconds}-second window")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().expect("valid")
    }

    fn parse(raw: &str) -> Result<Vec<QuotaWindow>, ProviderError> {
        let value: Value = serde_json::from_str(raw).expect("fixture is valid JSON");
        parse_usage(&value, now())
    }

    /// Values invented; no credentials appear in any fixture.
    const PLUS_FIXTURE: &str = r#"{
        "rate_limit": {
            "primary_window": {
                "used_percent": 37.5,
                "limit_window_seconds": 18000,
                "reset_after_seconds": 4800,
                "reset_at": 1700004800
            },
            "secondary_window": {
                "used_percent": 61.25,
                "limit_window_seconds": 604800,
                "reset_after_seconds": 432000,
                "reset_at": 1700432000
            }
        },
        "code_review_rate_limit": {
            "used_percent": 0.0,
            "limit_window_seconds": 0,
            "reset_after_seconds": 0,
            "reset_at": 0
        },
        "some_new_field": {"we": "ignore this"}
    }"#;

    #[test]
    fn parses_primary_and_secondary_windows_by_duration() {
        let windows = parse(PLUS_FIXTURE).expect("parses");
        let keys: Vec<&str> = windows.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["five_hour", "seven_day"]);

        assert_eq!(windows[0].used_percent, Some(37.5));
        assert_eq!(windows[0].remaining_percent, Some(62.5));
        assert_eq!(windows[0].window_seconds, Some(18_000));
        assert_eq!(windows[0].resets_in_seconds, Some(4_800));

        assert_eq!(windows[1].label, "Weekly");
        assert_eq!(windows[1].used_percent, Some(61.25));
    }

    #[test]
    fn empty_code_review_window_is_omitted_not_rendered_at_epoch_zero() {
        let windows = parse(PLUS_FIXTURE).expect("parses");
        assert!(windows.iter().all(|w| !w.key.starts_with("code_review_")));
        assert!(windows
            .iter()
            .all(|w| w.resets_at.map(|d| d.timestamp() > 0).unwrap_or(true)));
    }

    #[test]
    fn parses_official_app_server_rate_limits_for_isolated_profiles() {
        let value = serde_json::json!({
            "primary": {
                "usedPercent": 28,
                "windowDurationMins": 300,
                "resetsAt": 1_700_004_800_i64
            },
            "secondary": {
                "usedPercent": 63,
                "windowDurationMins": 10_080,
                "resetsAt": 1_700_432_000_i64
            },
            "planType": "plus"
        });
        let windows = parse_app_server_rate_limits(&value, now()).expect("parses");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].used_percent, Some(28.0));
        assert_eq!(windows[0].resets_in_seconds, Some(4_800));
        assert_eq!(windows[1].key, "seven_day");
        assert_eq!(windows[1].used_percent, Some(63.0));
    }

    #[test]
    fn a_free_plan_weekly_cap_in_the_primary_slot_is_labelled_weekly() {
        // Regression guard: naming by slot would have called this "5-hour".
        let raw = r#"{"rate_limit":{"primary_window":{
            "used_percent": 12.0, "limit_window_seconds": 604800,
            "reset_after_seconds": 100, "reset_at": 1700000100}}}"#;
        let windows = parse(raw).expect("parses");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "seven_day");
        assert_eq!(windows[0].label, "Weekly");
    }

    #[test]
    fn unknown_durations_still_surface_with_a_generated_key() {
        let raw = r#"{"rate_limit":{"primary_window":{
            "used_percent": 5.0, "limit_window_seconds": 7200,
            "reset_at": 1700000100}}}"#;
        let windows = parse(raw).expect("parses");
        assert_eq!(windows[0].key, "window_7200s");
        assert_eq!(windows[0].label, "2-hour window");
    }

    #[test]
    fn code_review_window_is_prefixed_and_labelled() {
        let raw = r#"{
            "rate_limit": {"primary_window": {"used_percent": 1.0, "limit_window_seconds": 18000, "reset_at": 1700000100}},
            "code_review_rate_limit": {"used_percent": 20.0, "limit_window_seconds": 604800, "reset_at": 1700432000}
        }"#;
        let windows = parse(raw).expect("parses");
        let review = windows
            .iter()
            .find(|w| w.key == "code_review_seven_day")
            .expect("code review window present");
        assert_eq!(review.label, "Code review · Weekly");
        assert_eq!(review.used_percent, Some(20.0));
    }

    #[test]
    fn duplicate_durations_keep_the_primary_window() {
        let raw = r#"{"rate_limit":{
            "primary_window": {"used_percent": 10.0, "limit_window_seconds": 18000, "reset_at": 1700000100},
            "secondary_window": {"used_percent": 90.0, "limit_window_seconds": 18000, "reset_at": 1700000100}
        }}"#;
        let windows = parse(raw).expect("parses");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, Some(10.0));
    }

    #[test]
    fn a_response_without_rate_limit_is_a_parse_error() {
        let err = parse(r#"{"something_else": true}"#).expect_err("no rate_limit");
        assert_eq!(err.kind, ProviderErrorKind::Parse);
        assert!(err.remediation.is_some());

        let err = parse("[]").expect_err("not an object");
        assert_eq!(err.kind, ProviderErrorKind::Parse);
    }

    #[test]
    fn a_brand_new_account_with_no_windows_is_not_an_error() {
        let windows = parse(r#"{"rate_limit":{}}"#).expect("empty is fine");
        assert!(windows.is_empty());
    }

    #[test]
    fn relative_reset_is_used_when_absolute_is_missing() {
        let raw = r#"{"rate_limit":{"primary_window":{
            "used_percent": 4.0, "limit_window_seconds": 18000, "reset_after_seconds": 600}}}"#;
        let windows = parse(raw).expect("parses");
        assert_eq!(windows[0].resets_in_seconds, Some(600));
        assert_eq!(
            windows[0].resets_at.map(|d| d.timestamp()),
            Some(1_700_000_600)
        );
    }

    #[test]
    fn window_naming_tolerates_five_percent_drift() {
        assert_eq!(window_naming(18_000).0, "five_hour");
        assert_eq!(window_naming(17_500).0, "five_hour");
        assert_eq!(window_naming(18_500).0, "five_hour");
        assert_eq!(window_naming(604_800).0, "seven_day");
        assert_eq!(window_naming(2_592_000).0, "thirty_day");
        assert_eq!(window_naming(0).0, "window");
        assert_eq!(window_naming(-5).0, "window");
    }

    #[test]
    fn revoked_token_errors_get_their_own_remediation() {
        let err = ProviderError::unauthorized(
            "chatgpt.com rejected the credential: {\"detail\":\"token_revoked\"}",
        );
        let annotated = annotate(err);
        assert_eq!(annotated.kind, ProviderErrorKind::Unauthorized);
        assert_eq!(annotated.remediation.as_deref(), Some(REVOKED_HINT));

        let err = ProviderError::unauthorized("chatgpt.com rejected the credential");
        assert_eq!(annotate(err).remediation.as_deref(), Some(LOGIN_HINT));

        // Non-auth errors are passed through untouched.
        let err = ProviderError::network("offline");
        assert_eq!(annotate(err).remediation, None);
    }

    #[test]
    fn plan_prettifier_handles_known_values() {
        assert_eq!(pretty_plan("plus"), "Plus");
        assert_eq!(pretty_plan("pro"), "Pro");
        assert_eq!(pretty_plan("chatgpt_team"), "Chatgpt Team");
        assert_eq!(pretty_plan(""), "");
    }

    #[test]
    fn account_identity_comes_from_upstream_account_or_subject_not_cli_slot() {
        let descriptor = AccountDescriptor::new(
            ProviderId::Codex,
            "codex-cli",
            crate::models::CredentialRef::CodexCli { path: None },
        );
        let mut hint = AccountHint {
            subject: Some("subject-123".to_string()),
            ..AccountHint::default()
        };
        let subject_id = stable_account_id(&descriptor, &hint);
        assert!(subject_id.starts_with("codex:"));
        assert_eq!(subject_id.len(), "codex:".len() + 64);

        hint.extra
            .insert("codex_account_id".to_string(), "acct-file".to_string());
        let file_id = stable_account_id(&descriptor, &hint);
        assert_ne!(file_id, subject_id);

        hint.extra.insert(
            "chatgpt_account_id".to_string(),
            "acct-upstream".to_string(),
        );
        let upstream_id = stable_account_id(&descriptor, &hint);
        assert_ne!(upstream_id, file_id);
    }

    #[test]
    fn humanize_duration_is_readable() {
        assert_eq!(humanize_duration(7_200), "2-hour window");
        assert_eq!(humanize_duration(3_600), "1-hour window");
        assert_eq!(humanize_duration(172_800), "2-day window");
        assert_eq!(humanize_duration(90), "90-second window");
    }

    #[test]
    fn capability_flags_the_api_key_limitation() {
        let capability = CodexProvider::capability_info();
        assert_eq!(capability.provider, ProviderId::Codex);
        assert!(capability.read_only);
        assert!(!capability.official_api);
        assert!(capability.notes.iter().any(|n| n.contains("API-key-only")));
    }
}
