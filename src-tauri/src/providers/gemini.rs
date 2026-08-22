//! Honest Gemini capability reporting.
//!
//! Gemini's public API can execute model requests, and Google documents the
//! active ceilings in AI Studio, but an API key does not expose an
//! account-wide "percent used" snapshot. Since that percentage is EyeUrAI's
//! core promise, this adapter reports the gap instead of estimating it.

use crate::models::{
    AccountDescriptor, AccountSnapshot, CapabilityLevel, CapabilityOption, CredentialKind,
    Freshness, ProviderCapability, ProviderId, ProviderSnapshot,
};

use super::error::ProviderError;
use super::{not_configured, BoxFuture, ProviderContext, QuotaProvider};

const DOC_URL: &str = "https://ai.google.dev/gemini-api/docs/rate-limits";
const CONNECT_HINT: &str =
    "Gemini percentages are not currently exposed through a supported API; open AI Studio to inspect them.";

pub struct GeminiProvider;

impl GeminiProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn capability_info() -> ProviderCapability {
        ProviderCapability {
            provider: ProviderId::Gemini,
            display_name: ProviderId::Gemini.display_name().to_string(),
            level: CapabilityLevel::Unsupported,
            data_source: "No supported account-wide usage-percentage API".to_string(),
            official_api: true,
            read_only: true,
            supports_multiple_accounts: true,
            supports_percent: false,
            supports_reset_times: false,
            supports_currency: false,
            credential_kinds: vec![CredentialKind::Env, CredentialKind::Keychain],
            option_keys: vec![CapabilityOption::new(
                "project_id",
                "Google Cloud project ID",
                false,
            )
            .with_example("my-ai-project")],
            notes: vec![
                "Gemini limits are per project and model, not simply per API key.".to_string(),
                "EyeUrAI does not send a billable test prompt or infer usage from local history."
                    .to_string(),
            ],
            doc_url: Some(DOC_URL.to_string()),
        }
    }
}

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaProvider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    fn capability(&self) -> ProviderCapability {
        Self::capability_info()
    }

    fn fetch<'a>(
        &'a self,
        _ctx: &'a ProviderContext,
        accounts: &'a [AccountDescriptor],
    ) -> BoxFuture<'a, ProviderSnapshot> {
        Box::pin(async move {
            if accounts.is_empty() {
                return not_configured(Self::capability_info(), CONNECT_HINT);
            }

            let error = ProviderError::unsupported(
                "Gemini does not expose current account-wide quota percentages through a supported API",
            )
            .with_remediation("Use Google AI Studio for current project limits; EyeUrAI will add this when Google exposes a safe read-only endpoint.")
            .with_doc_url(DOC_URL)
            .to_info();

            let rows = accounts
                .iter()
                .enumerate()
                .map(|(index, descriptor)| {
                    let mut account = AccountSnapshot::failed(
                        descriptor.id.clone(),
                        descriptor.fallback_label(),
                        error.clone(),
                    );
                    account.active = index == 0;
                    account.freshness = Freshness::none();
                    account
                })
                .collect();

            let mut snapshot = ProviderSnapshot::new(Self::capability_info()).with_accounts(rows);
            snapshot.freshness = Freshness::none();
            snapshot
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CredentialRef, ProviderStatus};

    #[test]
    fn capability_never_promises_a_percentage() {
        let capability = GeminiProvider::capability_info();
        assert_eq!(capability.level, CapabilityLevel::Unsupported);
        assert!(!capability.supports_percent);
    }

    #[tokio::test]
    async fn configured_account_is_explicitly_unsupported() {
        let provider = GeminiProvider::new();
        let ctx = ProviderContext::new().unwrap();
        let descriptor =
            AccountDescriptor::new(ProviderId::Gemini, "gemini-project", CredentialRef::None);
        let snapshot = provider.fetch(&ctx, &[descriptor]).await;
        assert_eq!(snapshot.status, ProviderStatus::Unsupported);
        assert!(snapshot.accounts[0].windows.is_empty());
    }
}
