//! Provider adapters for EyeUrAI.
//!
//! # Shape
//!
//! ```text
//!   AccountDescriptor  ──►  CredentialResolver  ──►  Secret (never serialized)
//!          │                                            │
//!          ▼                                            ▼
//!   QuotaProvider::fetch  ───────────────────────►  HTTPS GET (bearer)
//!          │
//!          ▼
//!   ProviderSnapshot  ──►  QuotaSnapshot  ──►  Tauri command  ──►  UI
//! ```
//!
//! # Rules every adapter follows
//!
//! 1. **Read-only.** No adapter writes to a credential store, rotates a token,
//!    or mutates any vendor state.
//! 2. **No secrets escape.** Tokens live in [`secret::Secret`], which is not
//!    `Serialize`, redacts in `Debug`, and is only readable through
//!    `expose()`. Error messages pass through [`error::scrub`].
//! 3. **Bounded time.** Every request has a deadline and every provider has an
//!    overall budget; a hung endpoint degrades one row, never the app.
//! 4. **Honest gaps.** When a number is not obtainable through an official,
//!    safe mechanism, the adapter returns
//!    [`crate::models::ProviderStatus::Unsupported`] with an explanation. It
//!    never estimates, extrapolates, or fabricates.
//! 5. **Actionable errors.** Every failure carries a
//!    [`crate::models::ProviderErrorKind`] and, where one exists, a single
//!    concrete remediation string.

pub mod claude;
pub mod codex;
pub mod credentials;
pub mod demo;
pub mod error;
pub mod gemini;
pub mod jwt;
pub mod openrouter;
pub mod secret;

pub mod http;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::models::{
    AccountDescriptor, DataSource, ProviderCapability, ProviderErrorInfo, ProviderId,
    ProviderSnapshot, ProviderStatus, QuotaSnapshot,
};

use self::credentials::{CredentialResolver, LocalCredentialResolver};
use self::error::ProviderError;
use self::http::{HttpClient, HttpConfig};

/// Boxed future alias. Used instead of `async_trait` so the crate needs no
/// extra dependency and `Cargo.toml` stays untouched.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Injectable clock so snapshot assembly is deterministic under test.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Default wall clock.
pub fn system_clock() -> Clock {
    Arc::new(Utc::now)
}

/// User-Agent strings sent to each upstream.
///
/// The default is honest: it identifies EyeUrAI and links the project so
/// endpoint owners can find us. Two of these endpoints (Anthropic's OAuth
/// usage route and ChatGPT's `wham/usage`) are first-party app routes; some
/// tools impersonate the vendor CLI to land in a more lenient rate-limit
/// bucket. EyeUrAI does not do that by default. The overrides exist so a user
/// who is being throttled can opt in explicitly and knowingly.
#[derive(Debug, Clone)]
pub struct UserAgents {
    pub claude: String,
    pub codex: String,
    pub openrouter: String,
    pub gemini: String,
}

impl Default for UserAgents {
    fn default() -> Self {
        let honest = format!("EyeUrAI/{}", env!("CARGO_PKG_VERSION"));
        UserAgents {
            claude: env_or("EYEURAI_CLAUDE_USER_AGENT", &honest),
            codex: env_or("EYEURAI_CODEX_USER_AGENT", &honest),
            openrouter: env_or("EYEURAI_OPENROUTER_USER_AGENT", &honest),
            gemini: env_or("EYEURAI_GEMINI_USER_AGENT", &honest),
        }
    }
}

impl UserAgents {
    pub fn for_provider(&self, id: ProviderId) -> &str {
        match id {
            ProviderId::Claude => &self.claude,
            ProviderId::Codex => &self.codex,
            ProviderId::OpenRouter => &self.openrouter,
            ProviderId::Gemini => &self.gemini,
        }
    }
}

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Everything an adapter needs, injected rather than constructed, so tests and
/// the main agent can substitute pieces.
pub struct ProviderContext {
    pub http: HttpClient,
    pub credentials: Arc<dyn CredentialResolver>,
    pub user_agents: UserAgents,
    pub clock: Clock,
    /// Wall-clock budget for one provider across all of its accounts.
    pub provider_budget: Duration,
    /// Age at which cached data is flagged stale in [`crate::models::Freshness`].
    pub stale_after_seconds: i64,
}

impl ProviderContext {
    pub fn new() -> Result<Self, ProviderError> {
        Ok(ProviderContext {
            http: HttpClient::new(HttpConfig::default())?,
            credentials: Arc::new(LocalCredentialResolver::new()),
            user_agents: UserAgents::default(),
            clock: system_clock(),
            provider_budget: http::DEFAULT_PROVIDER_BUDGET,
            stale_after_seconds: 300,
        })
    }

    pub fn with_credentials(mut self, resolver: Arc<dyn CredentialResolver>) -> Self {
        self.credentials = resolver;
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub fn user_agent(&self, id: ProviderId) -> &str {
        self.user_agents.for_provider(id)
    }
}

/// One provider backend.
///
/// `fetch` is total: it never returns `Err`. A failure is expressed inside the
/// returned [`ProviderSnapshot`] so one broken provider can never blank the
/// whole UI.
pub trait QuotaProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Static description; must perform no I/O.
    fn capability(&self) -> ProviderCapability;

    fn fetch<'a>(
        &'a self,
        ctx: &'a ProviderContext,
        accounts: &'a [AccountDescriptor],
    ) -> BoxFuture<'a, ProviderSnapshot>;
}

/// The set of adapters the app knows about.
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn QuotaProvider>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        ProviderRegistry::with_defaults()
    }
}

impl ProviderRegistry {
    pub fn with_defaults() -> Self {
        ProviderRegistry {
            providers: vec![
                Arc::new(claude::ClaudeProvider::new()),
                Arc::new(codex::CodexProvider::new()),
                Arc::new(openrouter::OpenRouterProvider::new()),
                Arc::new(gemini::GeminiProvider::new()),
            ],
        }
    }

    pub fn from_providers(providers: Vec<Arc<dyn QuotaProvider>>) -> Self {
        ProviderRegistry { providers }
    }

    pub fn get(&self, id: ProviderId) -> Option<Arc<dyn QuotaProvider>> {
        self.providers.iter().find(|p| p.id() == id).map(Arc::clone)
    }

    /// Static capability table, no I/O. Ordered by [`ProviderId::ALL`].
    pub fn capabilities(&self) -> Vec<ProviderCapability> {
        let mut caps: Vec<ProviderCapability> =
            self.providers.iter().map(|p| p.capability()).collect();
        caps.sort_by_key(|c| {
            ProviderId::ALL
                .iter()
                .position(|id| *id == c.provider)
                .unwrap_or(usize::MAX)
        });
        caps
    }

    /// Fetch every requested provider concurrently, each under its own budget.
    ///
    /// `only` restricts the run to a subset (used by "refresh just this row").
    /// Providers with no descriptor still appear, as `NotConfigured`, so the UI
    /// can offer a "connect" affordance.
    pub async fn refresh(
        self: &Arc<Self>,
        ctx: Arc<ProviderContext>,
        descriptors: Arc<Vec<AccountDescriptor>>,
        only: Option<Vec<ProviderId>>,
    ) -> QuotaSnapshot {
        let now = ctx.now();
        let mut handles = Vec::new();

        for provider in &self.providers {
            let id = provider.id();
            if let Some(only) = &only {
                if !only.contains(&id) {
                    continue;
                }
            }

            let provider = Arc::clone(provider);
            let ctx = Arc::clone(&ctx);
            let descriptors = Arc::clone(&descriptors);
            let budget = ctx.provider_budget;

            handles.push((
                id,
                tokio::spawn(async move {
                    let mine: Vec<AccountDescriptor> = descriptors
                        .iter()
                        .filter(|d| d.provider == id && d.enabled)
                        .cloned()
                        .collect();
                    match tokio::time::timeout(budget, provider.fetch(&ctx, &mine)).await {
                        Ok(snapshot) => snapshot,
                        Err(_) => ProviderSnapshot::new(provider.capability()).with_error(
                            ProviderError::timeout(format!(
                                "{} did not finish within {} seconds",
                                id.display_name(),
                                budget.as_secs()
                            ))
                            .to_info(),
                        ),
                    }
                }),
            ));
        }

        let mut snapshots = Vec::with_capacity(handles.len());
        for (id, handle) in handles {
            match handle.await {
                Ok(snapshot) => snapshots.push(snapshot),
                Err(_) => {
                    if let Some(provider) = self.get(id) {
                        snapshots.push(ProviderSnapshot::new(provider.capability()).with_error(
                            ProviderErrorInfo::new(
                                crate::models::ProviderErrorKind::Internal,
                                format!("the {} refresh task failed", id.display_name()),
                            ),
                        ));
                    }
                }
            }
        }

        snapshots.sort_by_key(|s| {
            ProviderId::ALL
                .iter()
                .position(|id| *id == s.provider)
                .unwrap_or(usize::MAX)
        });

        let source = if snapshots
            .iter()
            .any(|s| s.status != ProviderStatus::NotConfigured)
        {
            DataSource::Live
        } else {
            DataSource::None
        };

        QuotaSnapshot::new(now, source, snapshots)
    }
}

// ---------------------------------------------------------------------------
// Shared adapter helpers
// ---------------------------------------------------------------------------

/// Build the `NotConfigured` snapshot every adapter returns when it has no
/// descriptors. Keeps the copy consistent across providers.
pub(crate) fn not_configured(
    capability: ProviderCapability,
    remediation: &str,
) -> ProviderSnapshot {
    let display = capability.display_name.clone();
    ProviderSnapshot::new(capability).with_error(
        ProviderError::credentials_missing(format!("{display} is not connected yet"))
            .with_remediation(remediation)
            .to_info(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountSnapshot, CapabilityLevel, CredentialKind};

    struct StubProvider {
        id: ProviderId,
        status: ProviderStatus,
        delay: Duration,
    }

    impl StubProvider {
        fn capability_for(id: ProviderId) -> ProviderCapability {
            ProviderCapability {
                provider: id,
                display_name: id.display_name().to_string(),
                level: CapabilityLevel::Full,
                data_source: "stub".into(),
                official_api: true,
                read_only: true,
                supports_multiple_accounts: false,
                supports_percent: true,
                supports_reset_times: true,
                supports_currency: false,
                credential_kinds: vec![CredentialKind::None],
                option_keys: vec![],
                notes: vec![],
                doc_url: None,
            }
        }
    }

    impl QuotaProvider for StubProvider {
        fn id(&self) -> ProviderId {
            self.id
        }

        fn capability(&self) -> ProviderCapability {
            StubProvider::capability_for(self.id)
        }

        fn fetch<'a>(
            &'a self,
            _ctx: &'a ProviderContext,
            _accounts: &'a [AccountDescriptor],
        ) -> BoxFuture<'a, ProviderSnapshot> {
            Box::pin(async move {
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                let mut account = AccountSnapshot::new("stub", "stub");
                account.status = self.status;
                ProviderSnapshot::new(self.capability()).with_accounts(vec![account])
            })
        }
    }

    fn test_context() -> Arc<ProviderContext> {
        Arc::new(ProviderContext {
            http: HttpClient::new(HttpConfig::default()).expect("client builds"),
            credentials: Arc::new(LocalCredentialResolver::new()),
            user_agents: UserAgents::default(),
            clock: Arc::new(|| {
                use chrono::TimeZone;
                Utc.timestamp_opt(1_700_000_000, 0).single().expect("valid")
            }),
            provider_budget: Duration::from_millis(100),
            stale_after_seconds: 300,
        })
    }

    #[test]
    fn default_registry_exposes_every_known_provider() {
        let registry = ProviderRegistry::with_defaults();
        let caps = registry.capabilities();
        let ids: Vec<ProviderId> = caps.iter().map(|c| c.provider).collect();
        assert_eq!(ids, ProviderId::ALL.to_vec());
        // All adapters must be read-only.
        assert!(caps.iter().all(|c| c.read_only));
    }

    #[test]
    fn user_agent_is_honest_by_default() {
        let agents = UserAgents::default();
        // Only assert the honest default when no override is present in this
        // environment, so the test stays deterministic under CI overrides.
        if std::env::var("EYEURAI_CLAUDE_USER_AGENT").is_err() {
            assert!(agents.claude.starts_with("EyeUrAI/"));
            assert!(!agents.claude.contains("claude-code/"));
        }
    }

    #[tokio::test]
    async fn refresh_runs_providers_and_orders_results() {
        let registry = Arc::new(ProviderRegistry::from_providers(vec![
            Arc::new(StubProvider {
                id: ProviderId::Gemini,
                status: ProviderStatus::Ok,
                delay: Duration::ZERO,
            }),
            Arc::new(StubProvider {
                id: ProviderId::Claude,
                status: ProviderStatus::Ok,
                delay: Duration::ZERO,
            }),
        ]));

        let snapshot = registry
            .refresh(test_context(), Arc::new(vec![]), None)
            .await;

        let ids: Vec<ProviderId> = snapshot.providers.iter().map(|p| p.provider).collect();
        assert_eq!(ids, vec![ProviderId::Claude, ProviderId::Gemini]);
        assert_eq!(snapshot.source, DataSource::Live);
        assert_eq!(snapshot.generated_at.timestamp(), 1_700_000_000);
    }

    #[tokio::test]
    async fn refresh_honours_the_only_filter() {
        let registry = Arc::new(ProviderRegistry::from_providers(vec![
            Arc::new(StubProvider {
                id: ProviderId::Claude,
                status: ProviderStatus::Ok,
                delay: Duration::ZERO,
            }),
            Arc::new(StubProvider {
                id: ProviderId::Codex,
                status: ProviderStatus::Ok,
                delay: Duration::ZERO,
            }),
        ]));

        let snapshot = registry
            .refresh(
                test_context(),
                Arc::new(vec![]),
                Some(vec![ProviderId::Codex]),
            )
            .await;
        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.providers[0].provider, ProviderId::Codex);
    }

    #[tokio::test]
    async fn a_slow_provider_times_out_without_blocking_the_others() {
        let registry = Arc::new(ProviderRegistry::from_providers(vec![
            Arc::new(StubProvider {
                id: ProviderId::Claude,
                status: ProviderStatus::Ok,
                delay: Duration::from_secs(30),
            }),
            Arc::new(StubProvider {
                id: ProviderId::Codex,
                status: ProviderStatus::Ok,
                delay: Duration::ZERO,
            }),
        ]));

        let snapshot = registry
            .refresh(test_context(), Arc::new(vec![]), None)
            .await;

        let claude = snapshot.provider(ProviderId::Claude).expect("present");
        assert_eq!(claude.status, ProviderStatus::Error);
        assert_eq!(
            claude.error.as_ref().map(|e| e.kind),
            Some(crate::models::ProviderErrorKind::Timeout)
        );

        let codex = snapshot.provider(ProviderId::Codex).expect("present");
        assert_eq!(codex.status, ProviderStatus::Ok);
    }

    #[tokio::test]
    async fn descriptors_are_partitioned_by_provider() {
        struct CountingProvider {
            id: ProviderId,
            seen: std::sync::Mutex<Vec<String>>,
        }

        impl QuotaProvider for CountingProvider {
            fn id(&self) -> ProviderId {
                self.id
            }
            fn capability(&self) -> ProviderCapability {
                StubProvider::capability_for(self.id)
            }
            fn fetch<'a>(
                &'a self,
                _ctx: &'a ProviderContext,
                accounts: &'a [AccountDescriptor],
            ) -> BoxFuture<'a, ProviderSnapshot> {
                Box::pin(async move {
                    let mut seen = self.seen.lock().expect("lock");
                    seen.extend(accounts.iter().map(|a| a.id.clone()));
                    ProviderSnapshot::new(self.capability())
                })
            }
        }

        let counting = Arc::new(CountingProvider {
            id: ProviderId::OpenRouter,
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let registry = Arc::new(ProviderRegistry::from_providers(vec![
            Arc::clone(&counting) as Arc<dyn QuotaProvider>,
        ]));

        let descriptors = vec![
            AccountDescriptor::new(
                ProviderId::OpenRouter,
                "mine",
                crate::models::CredentialRef::Env {
                    var: "X".to_string(),
                },
            ),
            AccountDescriptor::new(
                ProviderId::Claude,
                "not-mine",
                crate::models::CredentialRef::ClaudeCli,
            ),
            {
                let mut disabled = AccountDescriptor::new(
                    ProviderId::OpenRouter,
                    "disabled",
                    crate::models::CredentialRef::Env {
                        var: "Y".to_string(),
                    },
                );
                disabled.enabled = false;
                disabled
            },
        ];

        let _ = registry
            .refresh(test_context(), Arc::new(descriptors), None)
            .await;

        let seen = counting.seen.lock().expect("lock").clone();
        assert_eq!(seen, vec!["mine".to_string()]);
    }
}
