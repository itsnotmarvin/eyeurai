//! Normalized, serde-friendly domain model for EyeUrAI quota monitoring.
//!
//! # Design rules
//!
//! * **No secrets ever live here.** Nothing in this module holds an access
//!   token, refresh token, API key, or raw credential blob. Credential material
//!   is referenced indirectly through [`CredentialRef`], which names *where* a
//!   secret lives, never *what* it is. Anything that actually holds a secret
//!   lives in `providers::secret::Secret`, which is deliberately
//!   non-`Serialize` and redacted in `Debug`.
//! * **Honest absence.** Every numeric field a provider may not be able to
//!   supply is an `Option`. Providers must return [`ProviderStatus::Unsupported`]
//!   rather than fabricate a value.
//! * **Wire format is snake_case.** Struct fields use serde defaults (already
//!   snake_case); enums are explicitly `rename_all = "snake_case"` so the
//!   contract does not drift if a variant is renamed.
//!
//! # TypeScript mirror (for the frontend)
//!
//! ```ts
//! type ProviderId = "claude" | "codex" | "openrouter" | "gemini";
//! type ProviderStatus =
//!   | "ok" | "partial" | "unauthorized" | "rate_limited"
//!   | "unsupported" | "error" | "not_configured";
//! interface QuotaSnapshot {
//!   schema_version: number;
//!   generated_at: string;            // RFC3339
//!   source: "live" | "cache" | "demo";
//!   providers: ProviderSnapshot[];
//! }
//! interface ProviderSnapshot {
//!   provider: ProviderId;
//!   display_name: string;
//!   status: ProviderStatus;
//!   accounts: AccountSnapshot[];
//!   error: ProviderErrorInfo | null;
//!   freshness: Freshness;
//!   capability: ProviderCapability;
//! }
//! interface AccountSnapshot {
//!   account_id: string;              // stable, non-secret
//!   label: string;                   // e.g. email or masked key label
//!   plan: string | null;
//!   active: boolean;
//!   status: ProviderStatus;
//!   windows: QuotaWindow[];
//!   error: ProviderErrorInfo | null;
//!   freshness: Freshness;
//! }
//! interface QuotaWindow {
//!   key: string; label: string; kind: WindowKind;
//!   used_percent: number | null; remaining_percent: number | null;
//!   used: Measure | null; limit: Measure | null; remaining: Measure | null;
//!   resets_at: string | null; resets_in_seconds: number | null;
//!   window_seconds: number | null; note: string | null;
//! }
//! ```

use std::collections::BTreeMap;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

/// Bumped whenever the shape below changes incompatibly. The frontend should
/// refuse to render (and show an "update required" state) on a mismatch.
pub const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Provider identity
// ---------------------------------------------------------------------------

/// Stable identifier for a supported provider backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// Anthropic Claude subscription (Claude Code / claude.ai OAuth session).
    Claude,
    /// OpenAI Codex CLI / ChatGPT subscription quota windows.
    #[serde(rename = "codex")]
    Codex,
    /// OpenRouter API key credits and limits.
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// Google Gemini quota / billing.
    Gemini,
}

impl ProviderId {
    /// All providers in the order the UI should display them.
    pub const ALL: [ProviderId; 4] = [
        ProviderId::Claude,
        ProviderId::Codex,
        ProviderId::OpenRouter,
        ProviderId::Gemini,
    ];

    /// Lowercase wire identifier. Matches the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::OpenRouter => "openrouter",
            ProviderId::Gemini => "gemini",
        }
    }

    /// Human readable name for menu-bar rows.
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude",
            ProviderId::Codex => "Codex / ChatGPT",
            ProviderId::OpenRouter => "OpenRouter",
            ProviderId::Gemini => "Gemini",
        }
    }

    /// Parse a wire identifier. Accepts a couple of friendly aliases so the
    /// frontend and CLI can be forgiving.
    pub fn parse(value: &str) -> Option<ProviderId> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Some(ProviderId::Claude),
            "codex" | "openai" | "chatgpt" => Some(ProviderId::Codex),
            "openrouter" | "open_router" | "open-router" => Some(ProviderId::OpenRouter),
            "gemini" | "google" | "google_ai" => Some(ProviderId::Gemini),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Status / error taxonomy
// ---------------------------------------------------------------------------

/// Coarse health of a provider or one of its accounts.
///
/// Distinct from [`ProviderErrorKind`]: status is what the UI paints, kind is
/// why. A provider can be `Partial` (some accounts fine, some failed) while an
/// individual account carries its own `Unauthorized`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Fresh, complete data.
    Ok,
    /// Some data obtained, but at least one account or window failed.
    Partial,
    /// Credentials were found but rejected (401/403).
    Unauthorized,
    /// Upstream throttled us; existing data (if any) may be stale.
    RateLimited,
    /// No safe, official, read-only mechanism exists for this configuration.
    /// The UI must show an explanation, never a fake number.
    Unsupported,
    /// Network / parse / internal failure.
    Error,
    /// No account descriptor is configured for this provider yet.
    NotConfigured,
}

impl ProviderStatus {
    /// Whether this status represents usable quota numbers.
    pub fn has_data(self) -> bool {
        matches!(self, ProviderStatus::Ok | ProviderStatus::Partial)
    }
}

/// Machine-actionable classification of a provider failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// No credential found in the configured location at all.
    CredentialsMissing,
    /// Credential present but rejected / expired (HTTP 401).
    Unauthorized,
    /// Credential valid but lacks the scope or entitlement (HTTP 403).
    Forbidden,
    /// HTTP 429 or provider-specific throttling.
    RateLimited,
    /// DNS/TLS/connect/read failure; offline.
    Network,
    /// Our own deadline elapsed.
    Timeout,
    /// HTTP 200 but the body did not match the expected shape.
    Parse,
    /// Upstream returned a status we do not model (e.g. 4xx other than above).
    Upstream,
    /// The requested data cannot be obtained safely/officially.
    Unsupported,
    /// Bug on our side.
    Internal,
}

impl ProviderErrorKind {
    /// Whether an immediate automatic retry is plausibly useful.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            ProviderErrorKind::RateLimited
                | ProviderErrorKind::Network
                | ProviderErrorKind::Timeout
                | ProviderErrorKind::Upstream
        )
    }

    /// Status the UI should paint when this is the dominant failure.
    pub fn to_status(self) -> ProviderStatus {
        match self {
            ProviderErrorKind::CredentialsMissing => ProviderStatus::NotConfigured,
            ProviderErrorKind::Unauthorized | ProviderErrorKind::Forbidden => {
                ProviderStatus::Unauthorized
            }
            ProviderErrorKind::RateLimited => ProviderStatus::RateLimited,
            ProviderErrorKind::Unsupported => ProviderStatus::Unsupported,
            ProviderErrorKind::Network
            | ProviderErrorKind::Timeout
            | ProviderErrorKind::Parse
            | ProviderErrorKind::Upstream
            | ProviderErrorKind::Internal => ProviderStatus::Error,
        }
    }
}

/// Serializable, user-facing description of a failure.
///
/// `message` is scrubbed of anything token-shaped before it reaches this
/// struct (see `providers::http::scrub`). `remediation` is the single concrete
/// action a user can take, e.g. "Run `claude /login`".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderErrorInfo {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    /// Optional documentation link the UI can render as "Learn more".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_url: Option<String>,
}

impl ProviderErrorInfo {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        ProviderErrorInfo {
            kind,
            message: message.into(),
            retryable: kind.is_retryable(),
            retry_after_seconds: None,
            remediation: None,
            doc_url: None,
        }
    }

    pub fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    pub fn with_retry_after(mut self, seconds: Option<u64>) -> Self {
        self.retry_after_seconds = seconds;
        self
    }

    pub fn with_doc_url(mut self, url: impl Into<String>) -> Self {
        self.doc_url = Some(url.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Units and measures
// ---------------------------------------------------------------------------

/// Unit attached to a [`Measure`]. Keeps percent-based providers (Claude,
/// Codex) and balance-based providers (OpenRouter) in one model without
/// pretending they are the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Percent,
    /// US dollars (OpenRouter credits are dollar-denominated).
    Usd,
    Credits,
    Requests,
    Tokens,
    Messages,
    Count,
    /// Unit is known to the provider but not to us; see `Measure::unit_label`.
    Unknown,
}

/// A quantity plus its unit. `unit_label` carries the provider's own wording
/// (e.g. `"requests/min/project"`) for display next to the number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub value: f64,
    pub unit: Unit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_label: Option<String>,
}

impl Measure {
    pub fn new(value: f64, unit: Unit) -> Self {
        Measure {
            value,
            unit,
            unit_label: None,
        }
    }

    pub fn usd(value: f64) -> Self {
        Measure::new(value, Unit::Usd)
    }

    pub fn percent(value: f64) -> Self {
        Measure::new(value, Unit::Percent)
    }

    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.unit_label = Some(label.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Quota windows
// ---------------------------------------------------------------------------

/// What sort of limit a window describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    /// A rolling usage window that refills at `resets_at` (Claude 5h/7d,
    /// Codex primary/secondary).
    Rolling,
    /// A fixed calendar period (monthly billing cycle).
    Calendar,
    /// A prepaid balance that only moves when topped up.
    Balance,
    /// A per-minute / per-second throughput cap, informational only.
    RateLimit,
    /// A hard configured ceiling with no consumption signal available.
    Allowance,
}

/// One normalized quota window.
///
/// Invariant: `used_percent` and `remaining_percent`, when both present, sum to
/// ~100. Either may be `None` when the provider only exposes one side (Cloud
/// Quotas exposes a limit with no consumption, OpenRouter exposes spend with no
/// cap on free-tier keys).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// Stable machine key, e.g. `five_hour`, `seven_day`, `credits`.
    pub key: String,
    /// Human label, e.g. `"5-hour session"`.
    pub label: String,
    pub kind: WindowKind,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_percent: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<Measure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Measure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<Measure>,

    /// Absolute reset instant, UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    /// Seconds until `resets_at`, clamped at zero. Computed at snapshot time so
    /// the frontend does not need to reason about clock skew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_in_seconds: Option<i64>,
    /// Nominal length of the window, when the provider states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<i64>,

    /// Free-text caveat for this window (e.g. "limit only; consumption is not
    /// exposed by this API").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl QuotaWindow {
    pub fn new(key: impl Into<String>, label: impl Into<String>, kind: WindowKind) -> Self {
        QuotaWindow {
            key: key.into(),
            label: label.into(),
            kind,
            used_percent: None,
            remaining_percent: None,
            used: None,
            limit: None,
            remaining: None,
            resets_at: None,
            resets_in_seconds: None,
            window_seconds: None,
            note: None,
        }
    }

    /// Sets `used_percent` (clamped to 0..=100) and derives `remaining_percent`.
    pub fn with_used_percent(mut self, used: f64) -> Self {
        let used = clamp_percent(used);
        self.used_percent = Some(used);
        self.remaining_percent = Some(round2(100.0 - used));
        self
    }

    pub fn with_used(mut self, used: Measure) -> Self {
        self.used = Some(used);
        self
    }

    pub fn with_limit(mut self, limit: Measure) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn with_remaining(mut self, remaining: Measure) -> Self {
        self.remaining = Some(remaining);
        self
    }

    pub fn with_window_seconds(mut self, seconds: i64) -> Self {
        self.window_seconds = Some(seconds);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Sets `resets_at` and recomputes `resets_in_seconds` relative to `now`.
    pub fn with_reset(mut self, resets_at: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        self.resets_at = Some(resets_at);
        self.resets_in_seconds = Some(seconds_until(resets_at, now));
        self
    }

    /// Recomputes `resets_in_seconds` from `resets_at` against a new `now`.
    /// Used when replaying a cached snapshot so countdowns stay truthful.
    pub fn refresh_countdown(&mut self, now: DateTime<Utc>) {
        if let Some(resets_at) = self.resets_at {
            self.resets_in_seconds = Some(seconds_until(resets_at, now));
        }
    }

    /// Derives percentages from `used`/`limit` when the provider gave absolute
    /// numbers instead. No-op when the limit is absent, zero, or negative.
    pub fn derive_percent_from_measures(mut self) -> Self {
        if self.used_percent.is_some() {
            return self;
        }
        let (Some(used), Some(limit)) = (self.used.as_ref(), self.limit.as_ref()) else {
            return self;
        };
        if limit.unit != used.unit || limit.value <= 0.0 || !limit.value.is_finite() {
            return self;
        }
        let pct = clamp_percent(used.value / limit.value * 100.0);
        self.used_percent = Some(pct);
        self.remaining_percent = Some(round2(100.0 - pct));
        self
    }
}

/// Clamp to `0.0..=100.0` and round to two decimals, mapping NaN to 0.
pub fn clamp_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    round2(value.clamp(0.0, 100.0))
}

/// Round to two decimal places, suppressing float artifacts in the wire format.
pub fn round2(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (value * 100.0).round() / 100.0
}

/// Whole seconds from `now` to `at`, never negative.
pub fn seconds_until(at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    let delta: ChronoDuration = at - now;
    delta.num_seconds().max(0)
}

// ---------------------------------------------------------------------------
// Freshness
// ---------------------------------------------------------------------------

/// Where a piece of data came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    /// Fetched from the upstream API during this refresh.
    Live,
    /// Served from the in-process cache.
    Cache,
    /// Synthetic data for UI development / screenshots. Never mixed with live.
    Demo,
    /// No fetch happened (not configured, unsupported).
    None,
}

/// How current a snapshot fragment is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Freshness {
    pub source: DataSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<i64>,
    /// True when `age_seconds` exceeds the provider's staleness budget.
    pub stale: bool,
}

impl Freshness {
    pub fn live(fetched_at: DateTime<Utc>) -> Self {
        Freshness {
            source: DataSource::Live,
            fetched_at: Some(fetched_at),
            age_seconds: Some(0),
            stale: false,
        }
    }

    pub fn cached(fetched_at: DateTime<Utc>, now: DateTime<Utc>, stale_after_seconds: i64) -> Self {
        let age = (now - fetched_at).num_seconds().max(0);
        Freshness {
            source: DataSource::Cache,
            fetched_at: Some(fetched_at),
            age_seconds: Some(age),
            stale: age > stale_after_seconds,
        }
    }

    pub fn none() -> Self {
        Freshness {
            source: DataSource::None,
            fetched_at: None,
            age_seconds: None,
            stale: false,
        }
    }

    pub fn demo(now: DateTime<Utc>) -> Self {
        Freshness {
            source: DataSource::Demo,
            fetched_at: Some(now),
            age_seconds: Some(0),
            stale: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Account descriptors (credential *references*, never credentials)
// ---------------------------------------------------------------------------

/// Where a credential can be read from. **Never contains the secret itself.**
///
/// The variants deliberately describe stores that already exist on the user's
/// machine (the Claude Code keychain item, `~/.codex/auth.json`) so EyeUrAI is
/// a read-only observer and does not become a second place secrets live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialRef {
    /// The credential written by `claude /login`:
    /// macOS Keychain item `Claude Code-credentials`, otherwise
    /// `~/.claude/.credentials.json`.
    ClaudeCli,
    /// The credential written by `codex login`: `~/.codex/auth.json`.
    /// `path` overrides the default location (useful for multi-profile setups).
    CodexCli {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// A process environment variable holding an API key.
    Env { var: String },
    /// A generic macOS Keychain generic-password item.
    Keychain {
        service: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
    /// An entry in the app's own `tauri-plugin-store` secret store. The main
    /// agent owns the store; providers only ever see the key name.
    AppStore { key: String },
    /// Explicitly no credential; used by descriptors that exist only to report
    /// an `Unsupported` state.
    None,
}

impl CredentialRef {
    /// Short, non-secret description for the settings UI.
    pub fn describe(&self) -> String {
        match self {
            CredentialRef::ClaudeCli => "Claude Code login (system credential store)".to_string(),
            CredentialRef::CodexCli { path } => match path {
                Some(p) => format!("Codex CLI credentials at {p}"),
                None => "Codex CLI login (~/.codex/auth.json)".to_string(),
            },
            CredentialRef::Env { var } => format!("Environment variable ${var}"),
            CredentialRef::Keychain { service, account } => match account {
                Some(a) => format!("Keychain item {service} ({a})"),
                None => format!("Keychain item {service}"),
            },
            CredentialRef::AppStore { key } => format!("EyeUrAI secure store entry \"{key}\""),
            CredentialRef::None => "No credential".to_string(),
        }
    }

    /// Coarse kind, used by [`ProviderCapability::credential_kinds`].
    pub fn kind(&self) -> CredentialKind {
        match self {
            CredentialRef::ClaudeCli => CredentialKind::ClaudeCli,
            CredentialRef::CodexCli { .. } => CredentialKind::CodexCli,
            CredentialRef::Env { .. } => CredentialKind::Env,
            CredentialRef::Keychain { .. } => CredentialKind::Keychain,
            CredentialRef::AppStore { .. } => CredentialKind::AppStore,
            CredentialRef::None => CredentialKind::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ClaudeCli,
    CodexCli,
    Env,
    Keychain,
    AppStore,
    None,
}

/// Identifies one provider account to poll.
///
/// This is the only handle a provider adapter is given. It is fully
/// serializable, safe to persist, and safe to log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDescriptor {
    pub provider: ProviderId,
    /// Stable local id. Must be unique within a provider. Not an upstream
    /// account id unless that id is itself non-sensitive.
    pub id: String,
    /// Display label chosen by the user, or `None` to let the adapter derive
    /// one from the upstream profile (e.g. account email).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub credential: CredentialRef,
    /// Non-secret provider parameters, e.g. `project_id` for Gemini Cloud
    /// Quotas or `region` filters. Adapters must reject anything secret here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
    /// Disabled descriptors are reported as `NotConfigured` without any I/O.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl AccountDescriptor {
    pub fn new(provider: ProviderId, id: impl Into<String>, credential: CredentialRef) -> Self {
        AccountDescriptor {
            provider,
            id: id.into(),
            label: None,
            credential,
            options: BTreeMap::new(),
            enabled: true,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }

    /// Best-effort display label that never requires network access.
    pub fn fallback_label(&self) -> String {
        self.label
            .clone()
            .unwrap_or_else(|| self.credential.describe())
    }

    pub fn option(&self, key: &str) -> Option<&str> {
        self.options.get(key).map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// How well a provider can be observed. Drives honest UI copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLevel {
    /// Usage percentages and reset times are obtainable.
    Full,
    /// Only some dimensions are obtainable (e.g. limits but not consumption).
    Partial,
    /// Nothing safely obtainable in the general case.
    Unsupported,
}

/// Static, no-I/O description of what a provider adapter can do. Returned by
/// the `provider_capabilities` command so the settings screen can be built
/// without a network round trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapability {
    pub provider: ProviderId,
    pub display_name: String,
    pub level: CapabilityLevel,
    /// Human description of the data source, e.g.
    /// "Anthropic OAuth usage endpoint (same call Claude Code makes)".
    pub data_source: String,
    /// True when the endpoint is documented and publicly supported by the
    /// vendor. False marks endpoints shared with a first-party app whose shape
    /// may change without notice.
    pub official_api: bool,
    /// This adapter performs no writes of any kind (no token rotation, no
    /// credential-store mutation). All four adapters are read-only.
    pub read_only: bool,
    pub supports_multiple_accounts: bool,
    pub supports_percent: bool,
    pub supports_reset_times: bool,
    pub supports_currency: bool,
    pub credential_kinds: Vec<CredentialKind>,
    /// Non-secret option keys the adapter understands, for the settings form.
    pub option_keys: Vec<CapabilityOption>,
    /// Caveats the UI should surface verbatim.
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityOption {
    pub key: String,
    pub label: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

impl CapabilityOption {
    pub fn new(key: &str, label: &str, required: bool) -> Self {
        CapabilityOption {
            key: key.to_string(),
            label: label.to_string(),
            required,
            example: None,
        }
    }

    pub fn with_example(mut self, example: &str) -> Self {
        self.example = Some(example.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// One account's quota state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    /// Mirrors [`AccountDescriptor::id`].
    pub account_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// True when this is the account the provider's CLI would currently use.
    pub active: bool,
    pub status: ProviderStatus,
    pub windows: Vec<QuotaWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderErrorInfo>,
    pub freshness: Freshness,
}

impl AccountSnapshot {
    pub fn new(account_id: impl Into<String>, label: impl Into<String>) -> Self {
        AccountSnapshot {
            account_id: account_id.into(),
            label: label.into(),
            plan: None,
            active: false,
            status: ProviderStatus::Ok,
            windows: Vec::new(),
            error: None,
            freshness: Freshness::none(),
        }
    }

    pub fn failed(
        account_id: impl Into<String>,
        label: impl Into<String>,
        error: ProviderErrorInfo,
    ) -> Self {
        let mut snapshot = AccountSnapshot::new(account_id, label);
        snapshot.status = error.kind.to_status();
        snapshot.error = Some(error);
        snapshot
    }

    /// Highest `used_percent` across windows; the number the tray icon shows.
    pub fn peak_used_percent(&self) -> Option<f64> {
        self.windows
            .iter()
            .filter_map(|w| w.used_percent)
            .fold(None, |acc: Option<f64>, pct| {
                Some(acc.map_or(pct, |a| a.max(pct)))
            })
    }

    /// Soonest reset across windows that carry one.
    pub fn next_reset_at(&self) -> Option<DateTime<Utc>> {
        self.windows.iter().filter_map(|w| w.resets_at).min()
    }
}

/// One provider's contribution to a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub provider: ProviderId,
    pub display_name: String,
    pub status: ProviderStatus,
    pub accounts: Vec<AccountSnapshot>,
    /// Provider-level failure (credential store unreadable, no descriptors,
    /// unsupported). Per-account failures live on the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderErrorInfo>,
    pub freshness: Freshness,
    pub capability: ProviderCapability,
}

impl ProviderSnapshot {
    pub fn new(capability: ProviderCapability) -> Self {
        ProviderSnapshot {
            provider: capability.provider,
            display_name: capability.display_name.clone(),
            status: ProviderStatus::Ok,
            accounts: Vec::new(),
            error: None,
            freshness: Freshness::none(),
            capability,
        }
    }

    pub fn with_error(mut self, error: ProviderErrorInfo) -> Self {
        self.status = error.kind.to_status();
        self.error = Some(error);
        self
    }

    pub fn with_accounts(mut self, accounts: Vec<AccountSnapshot>) -> Self {
        self.accounts = accounts;
        self.status = self.derive_status();
        self
    }

    /// Rolls per-account statuses up to a provider status.
    ///
    /// * any account OK and any account failing  -> `Partial`
    /// * all accounts share one failure status   -> that status
    /// * mixed failures, none OK                 -> `Error`
    /// * no accounts                             -> existing status
    pub fn derive_status(&self) -> ProviderStatus {
        if self.accounts.is_empty() {
            return self.status;
        }
        let ok = self.accounts.iter().filter(|a| a.status.has_data()).count();
        if ok == self.accounts.len() {
            return ProviderStatus::Ok;
        }
        if ok > 0 {
            return ProviderStatus::Partial;
        }
        let first = self.accounts[0].status;
        if self.accounts.iter().all(|a| a.status == first) {
            first
        } else {
            ProviderStatus::Error
        }
    }

    /// Sorts accounts: active first, then by label, then by id — deterministic
    /// output so the UI does not shuffle rows between refreshes.
    pub fn sort_accounts(&mut self) {
        self.accounts.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| a.label.cmp(&b.label))
                .then_with(|| a.account_id.cmp(&b.account_id))
        });
    }
}

/// Top-level payload returned to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub source: DataSource,
    pub providers: Vec<ProviderSnapshot>,
}

impl QuotaSnapshot {
    pub fn new(
        generated_at: DateTime<Utc>,
        source: DataSource,
        providers: Vec<ProviderSnapshot>,
    ) -> Self {
        QuotaSnapshot {
            schema_version: SCHEMA_VERSION,
            generated_at,
            source,
            providers,
        }
    }

    pub fn empty(generated_at: DateTime<Utc>) -> Self {
        QuotaSnapshot::new(generated_at, DataSource::None, Vec::new())
    }

    pub fn provider(&self, id: ProviderId) -> Option<&ProviderSnapshot> {
        self.providers.iter().find(|p| p.provider == id)
    }

    /// Highest `used_percent` across every account of every provider — the
    /// single number a menu-bar badge can show.
    pub fn peak_used_percent(&self) -> Option<f64> {
        self.providers
            .iter()
            .flat_map(|p| p.accounts.iter())
            .filter_map(AccountSnapshot::peak_used_percent)
            .fold(None, |acc: Option<f64>, pct| {
                Some(acc.map_or(pct, |a| a.max(pct)))
            })
    }

    /// Recomputes all countdowns against `now`. Call before returning a cached
    /// snapshot so reset timers do not freeze.
    pub fn refresh_countdowns(&mut self, now: DateTime<Utc>) {
        for provider in &mut self.providers {
            for account in &mut provider.accounts {
                for window in &mut account.windows {
                    window.refresh_countdown(now);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn provider_id_round_trips_through_serde() {
        for id in ProviderId::ALL {
            let json = serde_json::to_string(&id).expect("serialize");
            let back: ProviderId = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(id, back);
            assert_eq!(json, format!("\"{}\"", id.as_str()));
        }
    }

    #[test]
    fn provider_id_parses_aliases() {
        assert_eq!(ProviderId::parse("Anthropic"), Some(ProviderId::Claude));
        assert_eq!(ProviderId::parse(" chatgpt "), Some(ProviderId::Codex));
        assert_eq!(
            ProviderId::parse("open-router"),
            Some(ProviderId::OpenRouter)
        );
        assert_eq!(ProviderId::parse("nope"), None);
    }

    #[test]
    fn used_percent_clamps_and_derives_remaining() {
        let w =
            QuotaWindow::new("five_hour", "5-hour", WindowKind::Rolling).with_used_percent(133.333);
        assert_eq!(w.used_percent, Some(100.0));
        assert_eq!(w.remaining_percent, Some(0.0));

        let w = QuotaWindow::new("five_hour", "5-hour", WindowKind::Rolling)
            .with_used_percent(67.339_999_9);
        assert_eq!(w.used_percent, Some(67.34));
        assert_eq!(w.remaining_percent, Some(32.66));

        let w = QuotaWindow::new("x", "x", WindowKind::Rolling).with_used_percent(f64::NAN);
        assert_eq!(w.used_percent, Some(0.0));
    }

    #[test]
    fn reset_countdown_never_goes_negative() {
        let now = ts(1_000);
        let w = QuotaWindow::new("k", "k", WindowKind::Rolling).with_reset(ts(900), now);
        assert_eq!(w.resets_in_seconds, Some(0));

        let mut w = QuotaWindow::new("k", "k", WindowKind::Rolling).with_reset(ts(1_600), now);
        assert_eq!(w.resets_in_seconds, Some(600));
        w.refresh_countdown(ts(1_300));
        assert_eq!(w.resets_in_seconds, Some(300));
    }

    #[test]
    fn percent_derivation_requires_matching_units() {
        let derived = QuotaWindow::new("credits", "Credits", WindowKind::Balance)
            .with_used(Measure::usd(2.5))
            .with_limit(Measure::usd(10.0))
            .derive_percent_from_measures();
        assert_eq!(derived.used_percent, Some(25.0));
        assert_eq!(derived.remaining_percent, Some(75.0));

        let mismatched = QuotaWindow::new("credits", "Credits", WindowKind::Balance)
            .with_used(Measure::usd(2.5))
            .with_limit(Measure::new(10.0, Unit::Requests))
            .derive_percent_from_measures();
        assert_eq!(mismatched.used_percent, None);

        let zero_limit = QuotaWindow::new("credits", "Credits", WindowKind::Balance)
            .with_used(Measure::usd(2.5))
            .with_limit(Measure::usd(0.0))
            .derive_percent_from_measures();
        assert_eq!(zero_limit.used_percent, None);
    }

    #[test]
    fn error_kind_maps_to_status_and_retryability() {
        assert_eq!(
            ProviderErrorKind::Unauthorized.to_status(),
            ProviderStatus::Unauthorized
        );
        assert_eq!(
            ProviderErrorKind::CredentialsMissing.to_status(),
            ProviderStatus::NotConfigured
        );
        assert_eq!(
            ProviderErrorKind::Unsupported.to_status(),
            ProviderStatus::Unsupported
        );
        assert!(ProviderErrorKind::RateLimited.is_retryable());
        assert!(!ProviderErrorKind::Unauthorized.is_retryable());
        assert!(!ProviderErrorKind::Parse.is_retryable());
    }

    fn capability() -> ProviderCapability {
        ProviderCapability {
            provider: ProviderId::Claude,
            display_name: "Claude".into(),
            level: CapabilityLevel::Full,
            data_source: "test".into(),
            official_api: false,
            read_only: true,
            supports_multiple_accounts: true,
            supports_percent: true,
            supports_reset_times: true,
            supports_currency: false,
            credential_kinds: vec![CredentialKind::ClaudeCli],
            option_keys: vec![],
            notes: vec![],
            doc_url: None,
        }
    }

    fn account(id: &str, status: ProviderStatus) -> AccountSnapshot {
        let mut a = AccountSnapshot::new(id, id);
        a.status = status;
        a
    }

    #[test]
    fn provider_status_rolls_up_from_accounts() {
        let snap = ProviderSnapshot::new(capability()).with_accounts(vec![
            account("a", ProviderStatus::Ok),
            account("b", ProviderStatus::Ok),
        ]);
        assert_eq!(snap.status, ProviderStatus::Ok);

        let snap = ProviderSnapshot::new(capability()).with_accounts(vec![
            account("a", ProviderStatus::Ok),
            account("b", ProviderStatus::Unauthorized),
        ]);
        assert_eq!(snap.status, ProviderStatus::Partial);

        let snap = ProviderSnapshot::new(capability()).with_accounts(vec![
            account("a", ProviderStatus::Unauthorized),
            account("b", ProviderStatus::Unauthorized),
        ]);
        assert_eq!(snap.status, ProviderStatus::Unauthorized);

        let snap = ProviderSnapshot::new(capability()).with_accounts(vec![
            account("a", ProviderStatus::Unauthorized),
            account("b", ProviderStatus::RateLimited),
        ]);
        assert_eq!(snap.status, ProviderStatus::Error);
    }

    #[test]
    fn accounts_sort_active_first_then_label() {
        let mut snap = ProviderSnapshot::new(capability());
        let mut zed = AccountSnapshot::new("z", "zed@example.com");
        let mut abe = AccountSnapshot::new("a", "abe@example.com");
        let mut mid = AccountSnapshot::new("m", "mid@example.com");
        zed.active = false;
        abe.active = false;
        mid.active = true;
        snap.accounts = vec![zed, abe, mid];
        snap.sort_accounts();
        let order: Vec<&str> = snap
            .accounts
            .iter()
            .map(|a| a.account_id.as_str())
            .collect();
        assert_eq!(order, vec!["m", "a", "z"]);
    }

    #[test]
    fn peak_used_percent_spans_providers_and_accounts() {
        let mut a = AccountSnapshot::new("a", "a");
        a.windows = vec![
            QuotaWindow::new("w1", "w1", WindowKind::Rolling).with_used_percent(10.0),
            QuotaWindow::new("w2", "w2", WindowKind::Rolling).with_used_percent(91.5),
        ];
        let mut b = AccountSnapshot::new("b", "b");
        b.windows = vec![QuotaWindow::new("w1", "w1", WindowKind::Rolling).with_used_percent(42.0)];

        let mut provider = ProviderSnapshot::new(capability());
        provider.accounts = vec![a, b];
        let snapshot = QuotaSnapshot::new(ts(0), DataSource::Live, vec![provider]);
        assert_eq!(snapshot.peak_used_percent(), Some(91.5));
    }

    #[test]
    fn snapshot_serializes_without_any_credential_material() {
        let descriptor = AccountDescriptor::new(
            ProviderId::OpenRouter,
            "default",
            CredentialRef::Env {
                var: "OPENROUTER_API_KEY".into(),
            },
        );
        let json = serde_json::to_string(&descriptor).expect("serialize descriptor");
        assert!(json.contains("OPENROUTER_API_KEY"));
        assert!(json.contains("\"type\":\"env\""));
        // The descriptor names a variable, it never carries a value.
        assert!(!json.contains("sk-"));
    }

    #[test]
    fn freshness_marks_stale_beyond_budget() {
        let fetched = ts(1_000);
        let fresh = Freshness::cached(fetched, ts(1_030), 60);
        assert!(!fresh.stale);
        assert_eq!(fresh.age_seconds, Some(30));

        let stale = Freshness::cached(fetched, ts(1_200), 60);
        assert!(stale.stale);
        assert_eq!(stale.age_seconds, Some(200));
    }

    #[test]
    fn snapshot_json_shape_is_snake_case() {
        let mut account = AccountSnapshot::new("acct-1", "user@example.com");
        account.plan = Some("max".into());
        account.freshness = Freshness::live(ts(10));
        account.windows = vec![QuotaWindow::new("five_hour", "5-hour", WindowKind::Rolling)
            .with_used_percent(50.0)
            .with_reset(ts(100), ts(10))];
        let provider = ProviderSnapshot::new(capability()).with_accounts(vec![account]);
        let snapshot = QuotaSnapshot::new(ts(10), DataSource::Live, vec![provider]);

        let value = serde_json::to_value(&snapshot).expect("serialize");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["source"], "live");
        assert_eq!(value["providers"][0]["provider"], "claude");
        assert_eq!(value["providers"][0]["accounts"][0]["account_id"], "acct-1");
        assert_eq!(
            value["providers"][0]["accounts"][0]["windows"][0]["used_percent"],
            50.0
        );
        assert_eq!(
            value["providers"][0]["accounts"][0]["windows"][0]["resets_in_seconds"],
            90
        );
    }
}
