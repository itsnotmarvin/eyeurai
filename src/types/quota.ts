/**
 * EyeUrAI data contract.
 *
 * These types mirror the Rust snapshot that the Tauri backend serialises with
 * serde. The frontend normalises both `camelCase` and `snake_case` payloads
 * (see `src/lib/normalize.ts`) so the UI stays compatible whichever serde
 * rename convention the backend settles on.
 *
 * Reference Rust shape:
 *
 * ```rust
 * #[derive(Serialize)]
 * #[serde(rename_all = "camelCase")]
 * pub struct Snapshot {
 *     pub schema_version: u32,
 *     pub generated_at: DateTime<Utc>,
 *     pub refreshing: bool,
 *     pub accounts: Vec<Account>,
 * }
 * ```
 */

export const PROVIDER_IDS = ["claude", "openai", "openrouter", "gemini"] as const;

export type ProviderId = (typeof PROVIDER_IDS)[number];

/** Unit a quota window is measured in. Drives value formatting only. */
export type QuotaUnit =
  | "tokens"
  | "requests"
  | "messages"
  | "credits"
  | "usd"
  | "percent";

/** Cadence of a quota window. Drives copy and sort order only. */
export type QuotaKind =
  | "session"
  | "daily"
  | "weekly"
  | "monthly"
  | "credit"
  | "rate";

/** Freshness of a single account's data. */
export type AccountStatus = "fresh" | "stale" | "error" | "pending";

/** How the credential for an account was discovered. Never includes secrets. */
export type AuthSource = "oauth" | "api-key" | "cli" | "unknown";

/** A single stacked quota bar inside an account card. */
export interface QuotaWindow {
  /** Stable id, unique within the owning account. */
  id: string;
  /** Human label, e.g. "Session (5h)" or "Weekly". */
  label: string;
  kind: QuotaKind;
  unit: QuotaUnit;
  /** Amount consumed, in `unit`. */
  used: number;
  /** Total allowance in `unit`; `null` when the provider does not expose one. */
  limit: number | null;
  /** Authoritative 0–100 usage from the backend. */
  percentUsed: number;
  /** RFC 3339 timestamp of the next reset, or `null` when it never resets. */
  resetsAt: string | null;
  /** Optional short qualifier shown next to the bar label. */
  note: string | null;
}

/** One connected account. Multiple accounts per provider are expected. */
export interface Account {
  /** Stable id, unique across the whole snapshot. */
  id: string;
  provider: ProviderId;
  /** Display label such as an email, org name or CLI profile. */
  label: string;
  /** Subscription label, e.g. "Max 20x", "Pro", "Pay as you go". */
  plan: string | null;
  source: AuthSource;
  /**
   * Whether this is the account the provider CLI currently uses. Omitted for
   * legacy payloads and connections that do not expose a CLI-active identity.
   */
  isCliActive?: boolean;
  status: AccountStatus;
  /** Human readable detail for `stale` / `error` states. */
  message: string | null;
  /** RFC 3339 timestamp of the last successful read. */
  updatedAt: string | null;
  windows: QuotaWindow[];
}

/** Everything the popover renders in one shot. */
export interface Snapshot {
  schemaVersion: number;
  /** RFC 3339 timestamp for when the backend assembled this snapshot. */
  generatedAt: string;
  /** True while a background refresh is in flight. */
  refreshing: boolean;
  accounts: Account[];
}

export type LocalUsageProviderId = "claude" | "openai";

/** Token totals extracted from local CLI session logs. Never contains prompt text or paths. */
export interface LocalUsageProviderSummary {
  provider: LocalUsageProviderId;
  processedTokens: number;
  uncachedInputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  observations: number;
  sessions: number;
}

/** One provider's aggregate token activity for a UTC calendar day. */
export interface LocalUsageDaySummary {
  date: string;
  provider: LocalUsageProviderId;
  processedTokens: number;
}

/** Aggregate activity for one locally observed model. */
export interface LocalUsageModelSummary {
  provider: LocalUsageProviderId;
  model: string;
  processedTokens: number;
}

/** A read-only, device-local usage summary for a bounded rolling range. */
export interface LocalUsageSnapshot {
  generatedAt: string;
  rangeDays: number;
  processedTokens: number;
  uncachedInputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  observations: number;
  sessions: number;
  providers: LocalUsageProviderSummary[];
  daily: LocalUsageDaySummary[];
  models: LocalUsageModelSummary[];
}

/** Non-secret record kept when an account is disconnected from EyeUrAI. */
export interface DisconnectedAccount {
  id: string;
  provider: ProviderId;
  label: string;
}

/** Stable, non-secret reference to the exact quota shown in the macOS menu bar. */
export type PinnedQuotaDisplay = "usage" | "reset";

export interface PinnedQuota {
  accountId: string;
  windowId: string;
  /** Omitted for legacy/current usage pins; reset pins opt into the countdown. */
  display?: PinnedQuotaDisplay;
}

export type RefreshIntervalSeconds = 15 | 30 | 60 | 300;

export const APPEARANCE_THEMES = ["porcelain", "carbon", "lichen", "nocturne"] as const;
export type AppearanceTheme = (typeof APPEARANCE_THEMES)[number];

export const BACKGROUND_STYLES = ["solid", "material", "gradient", "photo"] as const;
export type BackgroundStyle = (typeof BACKGROUND_STYLES)[number];

/** Non-secret UI preferences. Persisted locally; never holds credentials. */
export interface Preferences {
  version: number;
  onboardingCompleted: boolean;
  enabledProviders: ProviderId[];
  notificationsEnabled: boolean;
  /** 0–100. Bars at or above this are amber. */
  warnThreshold: number;
  /** 0–100. Bars at or above this are red and trigger critical alerts. */
  criticalThreshold: number;
  /** Denser rows for people with many accounts. */
  compact: boolean;
  /** Complete visual language: palette, type, shape and data treatment. */
  appearanceTheme: AppearanceTheme;
  /** Independent backdrop treatment within the selected theme. */
  backgroundStyle: BackgroundStyle;
  /** Accounts EyeUrAI must not read. This never logs the provider itself out. */
  disconnectedAccounts: DisconnectedAccount[];
  /** Explicit consent to read usage counters from local Claude/Codex session logs. */
  localUsageEnabled: boolean;
  /** Exact account/window selected on Home for the macOS menu bar. */
  pinnedQuota: PinnedQuota | null;
  /** How often EyeUrAI re-reads live provider limits. */
  refreshIntervalSeconds: RefreshIntervalSeconds;
}

/** Severity derived from a percentage and the user's thresholds. */
export type Severity = "normal" | "warn" | "critical";

export interface ProviderMeta {
  id: ProviderId;
  name: string;
  /** Short subtitle used in the onboarding picker. */
  blurb: string;
  /** Accent colour used for marks, bars and focus rings. */
  accent: string;
}

export const PROVIDER_META: Record<ProviderId, ProviderMeta> = {
  claude: {
    id: "claude",
    name: "Claude",
    blurb: "Anthropic sessions & weekly limits",
    accent: "#d97757",
  },
  openai: {
    id: "openai",
    name: "OpenAI",
    blurb: "ChatGPT plans & Codex usage",
    accent: "#ededeb",
  },
  openrouter: {
    id: "openrouter",
    name: "OpenRouter",
    blurb: "Credit balance & rate limits",
    accent: "#a78bfa",
  },
  gemini: {
    id: "gemini",
    name: "Gemini",
    blurb: "Google AI plans & daily caps",
    accent: "#6ea8fe",
  },
};

export const PROVIDER_ORDER: readonly ProviderId[] = PROVIDER_IDS;
