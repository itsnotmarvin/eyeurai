//! Claude subscription usage.
//!
//! # Data source
//!
//! `GET https://api.anthropic.com/api/oauth/usage` with the OAuth access token
//! `claude /login` already stored, and the `anthropic-beta: oauth-2025-04-20`
//! header. This is the same call Claude Code makes to render `/usage`; it is a
//! first-party route rather than a documented public API, so its shape can
//! change without notice. The parser is written defensively for that:
//!
//! * the response is decoded as a loose JSON object, because Anthropic adds
//!   sibling fields (arrays, objects) over time and a strict struct would let
//!   one unknown field take the whole response down;
//! * a window whose shape is unexpected is skipped, not fatal;
//! * a window with no `resets_at` is skipped rather than rendered at epoch 0.
//!
//! `GET .../api/oauth/profile` supplies the account email and rate-limit tier
//! (the plan). It is best-effort: a failure downgrades the label, never the
//! quota numbers.
//!
//! # Two kinds of accounts
//!
//! The terminal login (`claude /login`) is read-only: this adapter never
//! refreshes that token, because Anthropic rotates the refresh token on use
//! and refreshing without writing the new one back would invalidate the
//! user's `claude` session. An expired terminal login is reported as
//! `Unauthorized` with a "run `claude /login`" remediation.
//!
//! Accounts added inside EyeUrAI are isolated profiles whose OAuth grant
//! EyeUrAI obtained itself (see [`crate::claude_profiles`]). Those are
//! refreshed by EyeUrAI inside their own profile, never touching the
//! terminal login, so several Claude accounts can stay live at once.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::models::{
    AccountDescriptor, AccountSnapshot, CapabilityLevel, CredentialKind, Freshness,
    ProviderCapability, ProviderId, ProviderSnapshot, QuotaWindow, WindowKind,
};

use super::error::ProviderError;
use super::http::JsonRequest;
use super::{not_configured, BoxFuture, ProviderContext, QuotaProvider};

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA_HEADER: &str = "oauth-2025-04-20";

const USAGE_TIMEOUT: Duration = Duration::from_secs(10);
const PROFILE_TIMEOUT: Duration = Duration::from_secs(5);

const LOGIN_HINT: &str = "Run `claude /login` in a terminal, then refresh.";
const MANAGED_HINT: &str = "Add the Claude account again in EyeUrAI settings.";

/// Windows we surface from the top level of the usage response, with their
/// display labels and nominal lengths.
const TOP_LEVEL_WINDOWS: [(&str, &str, i64); 4] = [
    ("five_hour", "5-hour session", 5 * 3600),
    ("seven_day", "Weekly", 7 * 86400),
    ("seven_day_opus", "Weekly (Opus)", 7 * 86400),
    ("seven_day_sonnet", "Weekly (Sonnet)", 7 * 86400),
];

pub struct ClaudeProvider;

impl ClaudeProvider {
    pub fn new() -> Self {
        ClaudeProvider
    }

    pub fn capability_info() -> ProviderCapability {
        ProviderCapability {
            provider: ProviderId::Claude,
            display_name: ProviderId::Claude.display_name().to_string(),
            level: CapabilityLevel::Full,
            data_source:
                "Anthropic OAuth usage endpoint, using the existing Claude Code login plus isolated EyeUrAI Claude accounts"
                    .to_string(),
            official_api: false,
            read_only: true,
            supports_multiple_accounts: true,
            supports_percent: true,
            supports_reset_times: true,
            supports_currency: false,
            credential_kinds: vec![CredentialKind::ClaudeCli, CredentialKind::Keychain],
            option_keys: vec![],
            notes: vec![
                "Reads the credential `claude /login` wrote; EyeUrAI never modifies it."
                    .to_string(),
                "This is a first-party endpoint, not a documented public API, so the reported windows can change."
                    .to_string(),
                "An expired terminal login is reported as such — EyeUrAI never refreshes it, because that would invalidate your `claude` session."
                    .to_string(),
                "Accounts added in EyeUrAI use Anthropic's official browser sign-in in an isolated profile; EyeUrAI refreshes only those grants, requested with a read-only scope."
                    .to_string(),
            ],
            doc_url: None,
        }
    }
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        ClaudeProvider::new()
    }
}

impl QuotaProvider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn capability(&self) -> ProviderCapability {
        ClaudeProvider::capability_info()
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a ProviderContext,
        accounts: &'a [AccountDescriptor],
    ) -> BoxFuture<'a, ProviderSnapshot> {
        Box::pin(async move {
            if accounts.is_empty() {
                return not_configured(ClaudeProvider::capability_info(), LOGIN_HINT);
            }

            let mut results =
                super::fetch_accounts_concurrently(accounts, |descriptor, is_primary| {
                    fetch_one(ctx, descriptor, is_primary)
                })
                .await;
            // A user may add the same account that their terminal login
            // happens to use. Prefer the independently managed profile in
            // that case, while retaining every distinct managed profile.
            super::prefer_managed_accounts(&mut results, "claude-profile:");

            let mut snapshot =
                ProviderSnapshot::new(ClaudeProvider::capability_info()).with_accounts(results);
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
    let now = ctx.now();
    let fallback_label = descriptor.fallback_label();
    let user_agent = ctx.user_agent(ProviderId::Claude);

    // EyeUrAI-owned profiles resolve (and refresh) through their profile
    // manager; the terminal login goes through the read-only resolver. An
    // isolated profile stays independently readable regardless of what the
    // terminal is signed into, so it is always an active row.
    let managed_home = descriptor.option("claude_home").map(str::to_string);
    let is_managed = managed_home.is_some();
    let is_active = is_primary || is_managed;
    let login_hint = if is_managed { MANAGED_HINT } else { LOGIN_HINT };

    let credential = match &managed_home {
        Some(home) => {
            crate::claude_profiles::resolve_credential(Path::new(home), &ctx.http, user_agent, now)
                .await
        }
        None => ctx.credentials.resolve(descriptor).await,
    };
    let credential = match credential {
        Ok(credential) => credential,
        Err(err) => {
            let err = if err.remediation.is_some() {
                err
            } else {
                err.with_remediation(login_hint)
            };
            let mut snapshot =
                AccountSnapshot::failed(descriptor.id.clone(), fallback_label, err.to_info());
            snapshot.active = is_active;
            return snapshot;
        }
    };

    if credential.is_expired(now) {
        let mut snapshot = AccountSnapshot::failed(
            descriptor.id.clone(),
            fallback_label,
            ProviderError::unauthorized("your Claude login has expired")
                .with_remediation(login_hint)
                .to_info(),
        );
        snapshot.active = is_active;
        return snapshot;
    }

    // Profile is best effort: it only improves the row's label and plan.
    let profile = ctx
        .http
        .get_json::<Value>(
            &JsonRequest::new(PROFILE_ENDPOINT, user_agent)
                .bearer(&credential.access_token)
                .header("anthropic-beta", BETA_HEADER)
                .timeout(PROFILE_TIMEOUT)
                .max_attempts(1),
        )
        .await
        .ok()
        .map(|value| parse_profile(&value));

    let usage = ctx
        .http
        .get_json::<Value>(
            &JsonRequest::new(USAGE_ENDPOINT, user_agent)
                .bearer(&credential.access_token)
                .header("anthropic-beta", BETA_HEADER)
                .timeout(USAGE_TIMEOUT),
        )
        .await;

    let label = profile
        .as_ref()
        .and_then(|p| p.email.clone())
        .or_else(|| credential.hint.email.clone())
        .or_else(|| descriptor.label.clone())
        .unwrap_or(fallback_label);

    let plan = profile
        .as_ref()
        .and_then(|p| p.plan.clone())
        .or_else(|| credential.hint.plan.clone());
    // The isolated profile is the durable connection identity (see the same
    // reasoning in the Codex adapter): a failure path only knows the
    // descriptor, so profile-derived IDs would split one connection into
    // duplicate rows the moment a read fails.
    let account_id = if is_managed {
        descriptor.id.clone()
    } else {
        stable_account_id(descriptor, profile.as_ref())
    };

    match usage {
        Ok(value) => match parse_usage(&value, now) {
            Ok(windows) => {
                let mut snapshot = AccountSnapshot::new(account_id.clone(), label);
                snapshot.plan = plan;
                snapshot.active = is_active;
                snapshot.windows = windows;
                snapshot.freshness = Freshness::live(now);
                snapshot
            }
            Err(err) => {
                let mut snapshot =
                    AccountSnapshot::failed(account_id.clone(), label, err.to_info());
                snapshot.plan = plan;
                snapshot.active = is_active;
                snapshot
            }
        },
        Err(err) => {
            let err = match err.kind {
                crate::models::ProviderErrorKind::Unauthorized => err.with_remediation(login_hint),
                _ => err,
            };
            let mut snapshot = AccountSnapshot::failed(account_id, label, err.to_info());
            snapshot.plan = plan;
            snapshot.active = is_active;
            snapshot
        }
    }
}

/// Non-secret identity fields from `/api/oauth/profile`.
#[derive(Debug, Default, PartialEq)]
pub struct ClaudeProfile {
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub plan: Option<String>,
}

/// Parse the profile response leniently: any missing piece simply stays `None`.
pub fn parse_profile(value: &Value) -> ClaudeProfile {
    let account = value.get("account");
    ClaudeProfile {
        account_id: account
            .and_then(|account| account.get("uuid").or_else(|| account.get("id")))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string()),
        email: account
            .and_then(|a| a.get("email"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        display_name: account
            .and_then(|a| a.get("display_name"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        plan: value
            .get("organization")
            .and_then(|o| o.get("rate_limit_tier"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(pretty_tier),
    }
}

/// Claude's profile UUID is preferred; normalized email is a stable fallback
/// for response variants that omit the UUID. Distinct workspace principals
/// can share an email address, so email must not merge UUID-backed accounts.
/// The fixed credential slot is used only when the best-effort profile call
/// yielded no principal information and is never persisted by the registry.
fn stable_account_id(descriptor: &AccountDescriptor, profile: Option<&ClaudeProfile>) -> String {
    profile
        .and_then(|profile| {
            profile
                .account_id
                .as_deref()
                .and_then(|identity| prefixed_identity("claude:", identity, false))
                .or_else(|| {
                    profile
                        .email
                        .as_deref()
                        .and_then(|email| prefixed_identity("claude:", email, true))
                })
        })
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

/// `claude_max_20x` -> `Max 20x`, `default_claude_pro` -> `Pro`.
fn pretty_tier(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_start_matches("default_")
        .trim_start_matches("claude_");
    if cleaned.is_empty() {
        return raw.to_string();
    }
    cleaned
        .split('_')
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

/// Parse the usage response into normalized windows.
///
/// Returns `Err` only when nothing usable could be extracted at all, which is
/// the signal that the upstream shape has changed in a way we must surface.
pub fn parse_usage(value: &Value, now: DateTime<Utc>) -> Result<Vec<QuotaWindow>, ProviderError> {
    let object = value
        .as_object()
        .ok_or_else(|| ProviderError::parse("the Claude usage response was not a JSON object"))?;

    let mut windows: Vec<QuotaWindow> = Vec::new();

    for (key, label, window_seconds) in TOP_LEVEL_WINDOWS {
        let Some(entry) = object.get(key).and_then(Value::as_object) else {
            continue;
        };
        let Some(utilization) = entry.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        let mut window = QuotaWindow::new(key, label, WindowKind::Rolling)
            .with_used_percent(utilization)
            .with_window_seconds(window_seconds);
        if let Some(resets_at) = entry
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339)
        {
            window = window.with_reset(resets_at, now);
        }
        windows.push(window);
    }

    // `limits` is Anthropic's newer, model-scoped weekly limits array. Entries
    // are additive: a malformed one is skipped so it can never take the
    // top-level windows down with it.
    if let Some(limits) = object.get("limits").and_then(Value::as_array) {
        for entry in limits {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            if entry.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let Some(percent) = entry.get("percent").and_then(Value::as_f64) else {
                continue;
            };
            let display_name = entry
                .get("scope")
                .and_then(|s| s.get("model"))
                .and_then(|m| m.get("display_name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(slug) = model_slug(display_name) else {
                continue;
            };
            let key = format!("seven_day_{slug}");
            if windows.iter().any(|w| w.key == key) {
                continue;
            }
            let mut window =
                QuotaWindow::new(key, format!("Weekly ({display_name})"), WindowKind::Rolling)
                    .with_used_percent(percent)
                    .with_window_seconds(7 * 86400);
            if let Some(resets_at) = entry
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339)
            {
                window = window.with_reset(resets_at, now);
            }
            windows.push(window);
        }
    }

    if windows.is_empty() {
        return Err(ProviderError::parse(
            "the Claude usage response contained no recognizable limit windows; the upstream API may have changed",
        )
        .with_remediation("Update EyeUrAI, or report this if it persists."));
    }

    Ok(windows)
}

/// `"Claude Opus 4.5"` -> `"claude_opus_4_5"`. Returns `None` when nothing
/// usable survives (empty or punctuation-only names).
fn model_slug(display_name: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending_separator = false;
    for ch in display_name.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('_');
            }
            out.push(ch);
            pending_separator = false;
        } else {
            pending_separator = true;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse an RFC 3339 timestamp, tolerating fractional seconds and any offset.
pub fn parse_rfc3339(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().expect("valid")
    }

    /// Response shaped like a real one, with values invented. No credentials
    /// appear anywhere in these fixtures.
    const USAGE_FIXTURE: &str = r#"{
        "five_hour": {"utilization": 42.5, "resets_at": "2023-11-14T23:33:20Z"},
        "seven_day": {"utilization": 12.0, "resets_at": "2023-11-19T22:13:20Z"},
        "seven_day_opus": {"utilization": 3.25, "resets_at": "2023-11-19T22:13:20Z"},
        "some_future_array_field": [{"unknown": true}],
        "limits": [
            {
                "kind": "weekly_scoped",
                "percent": 61.5,
                "resets_at": "2023-11-19T22:13:20Z",
                "scope": {"model": {"display_name": "Claude Opus 4.5"}},
                "is_active": false
            },
            {"kind": "session", "percent": 99.0},
            {"kind": "weekly_scoped", "percent": 5.0, "scope": {"model": {"display_name": "!!!"}}}
        ]
    }"#;

    fn parse_fixture(raw: &str) -> Result<Vec<QuotaWindow>, ProviderError> {
        let value: Value = serde_json::from_str(raw).expect("fixture is valid JSON");
        parse_usage(&value, now())
    }

    #[test]
    fn parses_the_documented_windows() {
        let windows = parse_fixture(USAGE_FIXTURE).expect("parses");
        let keys: Vec<&str> = windows.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "five_hour",
                "seven_day",
                "seven_day_opus",
                "seven_day_claude_opus_4_5"
            ]
        );

        let five_hour = &windows[0];
        assert_eq!(five_hour.used_percent, Some(42.5));
        assert_eq!(five_hour.remaining_percent, Some(57.5));
        assert_eq!(five_hour.window_seconds, Some(18_000));
        assert_eq!(five_hour.kind, WindowKind::Rolling);
        // 2023-11-14T23:33:20Z == 1700004800, now == 1700000000
        assert_eq!(five_hour.resets_in_seconds, Some(4_800));
    }

    #[test]
    fn scoped_limits_are_surfaced_with_a_model_slug() {
        let windows = parse_fixture(USAGE_FIXTURE).expect("parses");
        let scoped = windows
            .iter()
            .find(|w| w.key == "seven_day_claude_opus_4_5")
            .expect("scoped window present");
        assert_eq!(scoped.label, "Weekly (Claude Opus 4.5)");
        assert_eq!(scoped.used_percent, Some(61.5));
    }

    #[test]
    fn unknown_sibling_fields_do_not_break_parsing() {
        let raw = r#"{
            "five_hour": {"utilization": 1.0, "resets_at": "2023-11-14T23:33:20Z"},
            "per_app_usage": [{"app": "cli", "tokens": 5}],
            "new_object_field": {"nested": {"deep": [1,2,3]}}
        }"#;
        let windows = parse_fixture(raw).expect("parses");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "five_hour");
    }

    #[test]
    fn a_window_shaped_unexpectedly_is_skipped_not_fatal() {
        let raw = r#"{
            "five_hour": [1,2,3],
            "seven_day": {"utilization": "lots"},
            "seven_day_opus": {"utilization": 7.0, "resets_at": "2023-11-19T22:13:20Z"}
        }"#;
        let windows = parse_fixture(raw).expect("parses");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "seven_day_opus");
    }

    #[test]
    fn a_window_without_a_reset_still_reports_its_percentage() {
        let raw = r#"{"five_hour": {"utilization": 88.0}}"#;
        let windows = parse_fixture(raw).expect("parses");
        assert_eq!(windows[0].used_percent, Some(88.0));
        assert_eq!(windows[0].resets_at, None);
        assert_eq!(windows[0].resets_in_seconds, None);
    }

    #[test]
    fn an_unrecognizable_response_is_a_parse_error() {
        let err = parse_fixture(r#"{"totally": "different"}"#).expect_err("no windows");
        assert_eq!(err.kind, crate::models::ProviderErrorKind::Parse);
        assert!(err.remediation.is_some());

        let err = parse_fixture("[]").expect_err("not an object");
        assert_eq!(err.kind, crate::models::ProviderErrorKind::Parse);
    }

    #[test]
    fn a_malformed_scoped_entry_never_removes_top_level_windows() {
        let raw = r#"{
            "five_hour": {"utilization": 10.0, "resets_at": "2023-11-14T23:33:20Z"},
            "limits": "this should be an array"
        }"#;
        let windows = parse_fixture(raw).expect("parses");
        assert_eq!(windows.len(), 1);
    }

    #[test]
    fn scoped_entries_never_override_a_top_level_window() {
        let raw = r#"{
            "seven_day_opus": {"utilization": 10.0, "resets_at": "2023-11-19T22:13:20Z"},
            "limits": [{"kind":"weekly_scoped","percent":99.0,"scope":{"model":{"display_name":"opus"}}}]
        }"#;
        let windows = parse_fixture(raw).expect("parses");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, Some(10.0));
    }

    #[test]
    fn model_slug_is_defensive() {
        assert_eq!(
            model_slug("Claude Opus 4.5").as_deref(),
            Some("claude_opus_4_5")
        );
        assert_eq!(model_slug("  Sonnet  ").as_deref(), Some("sonnet"));
        assert_eq!(model_slug("!!!"), None);
        assert_eq!(model_slug(""), None);
    }

    #[test]
    fn profile_parsing_is_lenient() {
        let value: Value = serde_json::from_str(
            r#"{"account":{"uuid":"u-1","email":"dev@example.com","display_name":"Dev"},
                "organization":{"rate_limit_tier":"default_claude_max_20x"}}"#,
        )
        .expect("valid JSON");
        let profile = parse_profile(&value);
        assert_eq!(profile.account_id.as_deref(), Some("u-1"));
        assert_eq!(profile.email.as_deref(), Some("dev@example.com"));
        assert_eq!(profile.display_name.as_deref(), Some("Dev"));
        assert_eq!(profile.plan.as_deref(), Some("Max 20x"));

        // Personal accounts have no organization block.
        let value: Value =
            serde_json::from_str(r#"{"account":{"email":"solo@example.com"}}"#).expect("valid");
        let profile = parse_profile(&value);
        assert_eq!(profile.plan, None);

        assert_eq!(parse_profile(&Value::Null), ClaudeProfile::default());
    }

    #[test]
    fn account_identity_prefers_profile_id_and_falls_back_to_normalized_email() {
        let descriptor = AccountDescriptor::new(
            ProviderId::Claude,
            "claude-cli",
            crate::models::CredentialRef::ClaudeCli,
        );
        let profile = ClaudeProfile {
            account_id: Some("user-uuid".to_string()),
            email: Some("Dev@Example.COM".to_string()),
            ..ClaudeProfile::default()
        };
        let uuid_id = stable_account_id(&descriptor, Some(&profile));
        assert!(uuid_id.starts_with("claude:"));
        assert_eq!(uuid_id.len(), "claude:".len() + 64);

        let email_only = ClaudeProfile {
            email: Some("Dev@Example.COM".to_string()),
            ..ClaudeProfile::default()
        };
        let email_id = stable_account_id(&descriptor, Some(&email_only));
        let normalized_email_id = stable_account_id(
            &descriptor,
            Some(&ClaudeProfile {
                email: Some("dev@example.com".to_string()),
                ..ClaudeProfile::default()
            }),
        );
        assert_eq!(email_id, normalized_email_id);
        assert_ne!(uuid_id, email_id);

        let uuid_only_id = stable_account_id(
            &descriptor,
            Some(&ClaudeProfile {
                account_id: Some("user-uuid".to_string()),
                ..ClaudeProfile::default()
            }),
        );
        assert_eq!(uuid_only_id, uuid_id);
        assert_ne!(uuid_only_id, email_id);
        assert_eq!(stable_account_id(&descriptor, None), "claude-cli");
    }

    #[test]
    fn tier_prettifier_handles_known_shapes() {
        assert_eq!(pretty_tier("default_claude_max_5x"), "Max 5x");
        assert_eq!(pretty_tier("claude_pro"), "Pro");
        assert_eq!(pretty_tier("enterprise"), "Enterprise");
        assert_eq!(pretty_tier(""), "");
    }

    #[test]
    fn rfc3339_parsing_accepts_offsets_and_fractions() {
        assert_eq!(
            parse_rfc3339("2023-11-14T23:33:20.123456Z").map(|d| d.timestamp()),
            Some(1_700_004_800)
        );
        assert_eq!(
            parse_rfc3339("2023-11-14T18:33:20-05:00").map(|d| d.timestamp()),
            Some(1_700_004_800)
        );
        assert_eq!(parse_rfc3339("not a date"), None);
    }

    #[test]
    fn resets_in_the_past_clamp_to_zero() {
        let raw = r#"{"five_hour": {"utilization": 5.0, "resets_at": "2020-01-01T00:00:00Z"}}"#;
        let windows = parse_fixture(raw).expect("parses");
        assert_eq!(windows[0].resets_in_seconds, Some(0));
    }

    #[test]
    fn capability_is_honest_about_the_endpoint() {
        let capability = ClaudeProvider::capability_info();
        assert_eq!(capability.provider, ProviderId::Claude);
        assert!(capability.read_only);
        assert!(!capability.official_api);
        assert!(capability.level == CapabilityLevel::Full);
        assert!(capability
            .notes
            .iter()
            .any(|n| n.contains("not a documented public API")));
    }
}
