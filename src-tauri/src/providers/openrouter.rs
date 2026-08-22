//! OpenRouter API-key spend limits.
//!
//! The current-key endpoint is public and documented by OpenRouter. It reports
//! the key's spend, optional USD ceiling, remaining allowance, and reset
//! cadence. EyeUrAI only performs this read-only GET; it never creates,
//! rotates, or edits a key.

use chrono::{Datelike, Duration as ChronoDuration, TimeZone, Utc};
use serde::Deserialize;

use crate::models::{
    AccountDescriptor, AccountSnapshot, CapabilityLevel, CredentialKind, Freshness, Measure,
    ProviderCapability, ProviderId, ProviderSnapshot, QuotaWindow, WindowKind,
};

use super::error::ProviderError;
use super::http::JsonRequest;
use super::{not_configured, BoxFuture, ProviderContext, QuotaProvider};

const KEY_ENDPOINT: &str = "https://openrouter.ai/api/v1/key";
const DOC_URL: &str = "https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key";
const CONNECT_HINT: &str = "Set OPENROUTER_API_KEY before launching EyeUrAI, then refresh.";

pub struct OpenRouterProvider;

impl OpenRouterProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn capability_info() -> ProviderCapability {
        ProviderCapability {
            provider: ProviderId::OpenRouter,
            display_name: ProviderId::OpenRouter.display_name().to_string(),
            level: CapabilityLevel::Full,
            data_source: "OpenRouter's documented current API-key endpoint".to_string(),
            official_api: true,
            read_only: true,
            supports_multiple_accounts: true,
            supports_percent: true,
            supports_reset_times: true,
            supports_currency: true,
            credential_kinds: vec![CredentialKind::Env, CredentialKind::Keychain],
            option_keys: vec![],
            notes: vec![
                "Usage and limits apply to the connected API key, not every key in the OpenRouter account."
                    .to_string(),
                "Keys without a spending ceiling can show spend but cannot produce a meaningful percentage."
                    .to_string(),
            ],
            doc_url: Some(DOC_URL.to_string()),
        }
    }
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaProvider for OpenRouterProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenRouter
    }

    fn capability(&self) -> ProviderCapability {
        Self::capability_info()
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a ProviderContext,
        accounts: &'a [AccountDescriptor],
    ) -> BoxFuture<'a, ProviderSnapshot> {
        Box::pin(async move {
            if accounts.is_empty() {
                return not_configured(Self::capability_info(), CONNECT_HINT);
            }

            let results = super::fetch_accounts_concurrently(accounts, |descriptor, is_primary| {
                fetch_one(ctx, descriptor, is_primary)
            })
            .await;

            let mut snapshot =
                ProviderSnapshot::new(Self::capability_info()).with_accounts(results);
            snapshot.freshness = Freshness::live(ctx.now());
            snapshot.sort_accounts();
            snapshot
        })
    }
}

#[derive(Debug, Deserialize)]
struct KeyEnvelope {
    data: KeyData,
}

#[derive(Debug, Deserialize)]
struct KeyData {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    is_free_tier: bool,
    #[serde(default)]
    usage: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    limit_remaining: Option<f64>,
    #[serde(default)]
    limit_reset: Option<String>,
}

async fn fetch_one(
    ctx: &ProviderContext,
    descriptor: &AccountDescriptor,
    is_primary: bool,
) -> AccountSnapshot {
    let now = ctx.now();
    let fallback_label = descriptor.fallback_label();
    let credential = match ctx.credentials.resolve(descriptor).await {
        Ok(credential) => credential,
        Err(error) => {
            return AccountSnapshot::failed(
                descriptor.id.clone(),
                fallback_label,
                error.with_remediation(CONNECT_HINT).to_info(),
            );
        }
    };

    let response = ctx
        .http
        .get_json::<KeyEnvelope>(
            &JsonRequest::new(KEY_ENDPOINT, ctx.user_agent(ProviderId::OpenRouter))
                .bearer(&credential.access_token),
        )
        .await;

    match response {
        Ok(envelope) => {
            let label = descriptor
                .label
                .clone()
                .or(envelope.data.label.clone())
                .unwrap_or(fallback_label);
            let mut account = AccountSnapshot::new(descriptor.id.clone(), label);
            account.plan = Some(if envelope.data.is_free_tier {
                "Free tier".to_string()
            } else {
                "API key".to_string()
            });
            account.active = is_primary;
            account.windows = parse_windows(&envelope.data, now);
            account.freshness = Freshness::live(now);
            account
        }
        Err(error) => {
            let mut account = AccountSnapshot::failed(
                descriptor.id.clone(),
                fallback_label,
                annotate(error).to_info(),
            );
            account.active = is_primary;
            account
        }
    }
}

fn annotate(error: ProviderError) -> ProviderError {
    match error.kind {
        crate::models::ProviderErrorKind::Unauthorized => error.with_remediation(
            "Check that this OpenRouter key is active, then reconnect it in EyeUrAI.",
        ),
        _ => error,
    }
}

fn parse_windows(data: &KeyData, now: chrono::DateTime<Utc>) -> Vec<QuotaWindow> {
    let used = data.usage.unwrap_or(0.0).max(0.0);
    let cadence = data.limit_reset.as_deref().map(str::to_ascii_lowercase);
    let label = match cadence.as_deref() {
        Some("daily") => "Daily spend limit",
        Some("weekly") => "Weekly spend limit",
        Some("monthly") => "Monthly spend limit",
        _ => "API key spend",
    };
    let kind = if cadence.is_some() {
        WindowKind::Calendar
    } else if data.limit.is_some() {
        WindowKind::Allowance
    } else {
        WindowKind::Balance
    };

    let mut window = QuotaWindow::new("spend_limit", label, kind).with_used(Measure::usd(used));
    if let Some(limit) = data
        .limit
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        window = window.with_limit(Measure::usd(limit));
    }
    if let Some(remaining) = data
        .limit_remaining
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        window = window.with_remaining(Measure::usd(remaining));
    }
    window = window.derive_percent_from_measures();

    if let Some(reset) = next_reset(cadence.as_deref(), now) {
        window = window.with_reset(reset, now);
    }
    if data.limit.is_none() {
        window = window.with_note("No spending ceiling is set for this key");
    }

    vec![window]
}

fn next_reset(cadence: Option<&str>, now: chrono::DateTime<Utc>) -> Option<chrono::DateTime<Utc>> {
    let today = now.date_naive();
    match cadence {
        Some("daily") => Utc
            .from_utc_datetime(&today.and_hms_opt(0, 0, 0)?)
            .checked_add_signed(ChronoDuration::days(1)),
        Some("weekly") => {
            let days = i64::from(now.weekday().num_days_from_monday());
            let monday = today.checked_sub_signed(ChronoDuration::days(days))?;
            Utc.from_utc_datetime(&monday.and_hms_opt(0, 0, 0)?)
                .checked_add_signed(ChronoDuration::days(7))
        }
        Some("monthly") => {
            let (year, month) = if now.month() == 12 {
                (now.year() + 1, 1)
            } else {
                (now.year(), now.month() + 1)
            };
            Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_key_produces_an_authoritative_percentage() {
        let now = Utc.with_ymd_and_hms(2026, 8, 9, 18, 0, 0).unwrap();
        let data = KeyData {
            label: Some("work".into()),
            is_free_tier: false,
            usage: Some(25.5),
            limit: Some(100.0),
            limit_remaining: Some(74.5),
            limit_reset: Some("monthly".into()),
        };
        let windows = parse_windows(&data, now);
        assert_eq!(windows[0].used_percent, Some(25.5));
        assert_eq!(windows[0].remaining_percent, Some(74.5));
        assert_eq!(
            windows[0].resets_at,
            Some(Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn uncapped_key_does_not_invent_a_percentage() {
        let now = Utc.with_ymd_and_hms(2026, 8, 9, 18, 0, 0).unwrap();
        let data = KeyData {
            label: None,
            is_free_tier: true,
            usage: Some(3.25),
            limit: None,
            limit_remaining: None,
            limit_reset: None,
        };
        let window = parse_windows(&data, now).remove(0);
        assert_eq!(window.used_percent, None);
        assert!(window.note.unwrap().contains("No spending ceiling"));
    }
}
