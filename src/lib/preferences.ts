import {
  APPEARANCE_THEMES,
  BACKGROUND_STYLES,
  PROVIDER_IDS,
  type AppearanceTheme,
  type BackgroundStyle,
  type DisconnectedAccount,
  type Preferences,
  type ProviderId,
  type RefreshIntervalSeconds,
} from "../types/quota";

/**
 * Non-secret UI preferences.
 *
 * Only display choices live here: which providers to show, notification opt-in
 * and the warn/critical thresholds, plus the stable IDs of a quota explicitly
 * pinned to the menu bar. Credentials, tokens and account payloads are owned by
 * the Rust side (keychain / OS credential store) and must never be written to
 * `localStorage`.
 */

export const PREFERENCES_KEY = "eyeurai.preferences.v1";
export const PREFERENCES_VERSION = 2;

export const REFRESH_INTERVAL_OPTIONS: ReadonlyArray<{
  value: RefreshIntervalSeconds;
  label: string;
}> = [
  { value: 15, label: "Near instant · 15 sec" },
  { value: 30, label: "Every 30 seconds" },
  { value: 60, label: "Every minute" },
  { value: 300, label: "Every 5 minutes" },
];

export const DEFAULT_PREFERENCES: Preferences = {
  version: PREFERENCES_VERSION,
  onboardingCompleted: false,
  launchAtLoginConfigured: false,
  enabledProviders: [...PROVIDER_IDS],
  notificationsEnabled: false,
  warnThreshold: 75,
  criticalThreshold: 90,
  compact: false,
  appearanceTheme: "porcelain",
  backgroundStyle: "solid",
  disconnectedAccounts: [],
  localUsageEnabled: false,
  pinnedQuota: null,
  refreshIntervalSeconds: 60,
};

export const THRESHOLD_MIN = 0;
export const THRESHOLD_MAX = 100;

function clampThreshold(value: unknown, fallback: number): number {
  const numeric = typeof value === "number" && Number.isFinite(value) ? value : fallback;
  return Math.min(THRESHOLD_MAX, Math.max(THRESHOLD_MIN, Math.round(numeric)));
}

function sanitizeProviders(value: unknown): ProviderId[] {
  if (!Array.isArray(value)) return [...PROVIDER_IDS];
  const seen = new Set<ProviderId>();
  for (const entry of value) {
    const match = PROVIDER_IDS.find((id) => id === entry);
    if (match) seen.add(match);
  }
  // Preserve the canonical provider order regardless of stored order.
  return PROVIDER_IDS.filter((id) => seen.has(id));
}

function sanitizeAppearanceTheme(value: unknown): AppearanceTheme {
  return (
    APPEARANCE_THEMES.find((theme) => theme === value) ?? DEFAULT_PREFERENCES.appearanceTheme
  );
}

function sanitizeBackgroundStyle(value: unknown): BackgroundStyle {
  return (
    BACKGROUND_STYLES.find((style) => style === value) ?? DEFAULT_PREFERENCES.backgroundStyle
  );
}

function sanitizeDisconnectedAccounts(value: unknown): DisconnectedAccount[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const accounts: DisconnectedAccount[] = [];
  for (const entry of value) {
    if (typeof entry !== "object" || entry === null) continue;
    const candidate = entry as Partial<DisconnectedAccount>;
    const provider = PROVIDER_IDS.find((id) => id === candidate.provider);
    const id = typeof candidate.id === "string" ? candidate.id.trim() : "";
    const label = typeof candidate.label === "string" ? candidate.label.trim() : "";
    if (!provider || !id || id.length > 160 || seen.has(id)) continue;
    seen.add(id);
    accounts.push({ id, provider, label: label.slice(0, 160) || "Connected account" });
  }
  return accounts;
}

const PINNED_QUOTA_ID_MAX_LENGTH = 160;

function sanitizePinnedQuotaId(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const id = value.trim();
  if (!id || id.length > PINNED_QUOTA_ID_MAX_LENGTH) return null;
  // Stable backend IDs are display-safe strings, never control-character payloads.
  if (/[\u0000-\u001f\u007f]/u.test(id)) return null;
  return id;
}

function sanitizePinnedQuota(value: unknown): Preferences["pinnedQuota"] {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const candidate = value as Record<string, unknown>;
  const accountId = sanitizePinnedQuotaId(candidate.accountId);
  const windowId = sanitizePinnedQuotaId(candidate.windowId);
  if (!accountId || !windowId) return null;
  return candidate.display === "reset"
    ? { accountId, windowId, display: "reset" }
    : { accountId, windowId };
}

function sanitizeRefreshInterval(value: unknown): RefreshIntervalSeconds {
  return (
    REFRESH_INTERVAL_OPTIONS.find((option) => option.value === value)?.value ??
    DEFAULT_PREFERENCES.refreshIntervalSeconds
  );
}

/** Coerces anything read from storage into a valid `Preferences`. */
export function sanitizePreferences(raw: unknown): Preferences {
  if (typeof raw !== "object" || raw === null) return { ...DEFAULT_PREFERENCES };
  const input = raw as Partial<Preferences>;

  const warnThreshold = clampThreshold(input.warnThreshold, DEFAULT_PREFERENCES.warnThreshold);
  const criticalThreshold = clampThreshold(
    input.criticalThreshold,
    DEFAULT_PREFERENCES.criticalThreshold,
  );

  return {
    version: PREFERENCES_VERSION,
    onboardingCompleted: input.onboardingCompleted === true,
    // Existing users may already have deliberately disabled the login item.
    // Treat their earlier preferences as configured so an update never reverses
    // that OS-level choice; only a genuinely fresh install receives the default.
    launchAtLoginConfigured:
      input.launchAtLoginConfigured === true ||
      (typeof input.version === "number" && input.version < PREFERENCES_VERSION) ||
      input.onboardingCompleted === true,
    enabledProviders: sanitizeProviders(input.enabledProviders),
    notificationsEnabled: input.notificationsEnabled === true,
    warnThreshold: Math.min(warnThreshold, criticalThreshold),
    criticalThreshold: Math.max(criticalThreshold, warnThreshold),
    compact: input.compact === true,
    appearanceTheme: sanitizeAppearanceTheme(input.appearanceTheme),
    backgroundStyle: sanitizeBackgroundStyle(input.backgroundStyle),
    disconnectedAccounts: sanitizeDisconnectedAccounts(input.disconnectedAccounts),
    localUsageEnabled: input.localUsageEnabled === true,
    pinnedQuota: sanitizePinnedQuota(input.pinnedQuota),
    refreshIntervalSeconds: sanitizeRefreshInterval(input.refreshIntervalSeconds),
  };
}

/** Keeps warn at or below critical while allowing the full 0–100 range. */
export function reconcileThresholds(
  next: Preferences,
  changed: "warn" | "critical",
): Preferences {
  const warn = clampThreshold(next.warnThreshold, DEFAULT_PREFERENCES.warnThreshold);
  const critical = clampThreshold(next.criticalThreshold, DEFAULT_PREFERENCES.criticalThreshold);

  if (changed === "warn") {
    return {
      ...next,
      warnThreshold: warn,
      criticalThreshold: Math.max(critical, warn),
    };
  }
  return {
    ...next,
    criticalThreshold: critical,
    warnThreshold: Math.min(warn, critical),
  };
}

type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** Used when `localStorage` is unavailable (private mode, hardened webview). */
const memory = new Map<string, string>();

const memoryStorage: StorageLike = {
  getItem: (key) => memory.get(key) ?? null,
  setItem: (key, value) => {
    memory.set(key, value);
  },
  removeItem: (key) => {
    memory.delete(key);
  },
};

function storage(): StorageLike {
  try {
    const candidate: Partial<StorageLike> | undefined =
      typeof window === "undefined" ? undefined : window.localStorage;
    if (
      candidate &&
      typeof candidate.getItem === "function" &&
      typeof candidate.setItem === "function" &&
      typeof candidate.removeItem === "function"
    ) {
      return candidate as StorageLike;
    }
  } catch {
    /* accessing localStorage can throw when storage is blocked */
  }
  return memoryStorage;
}

export function loadPreferences(): Preferences {
  try {
    const raw = storage().getItem(PREFERENCES_KEY);
    if (!raw) return { ...DEFAULT_PREFERENCES };
    return sanitizePreferences(JSON.parse(raw));
  } catch {
    return { ...DEFAULT_PREFERENCES };
  }
}

export function savePreferences(preferences: Preferences): void {
  try {
    storage().setItem(PREFERENCES_KEY, JSON.stringify(sanitizePreferences(preferences)));
  } catch {
    /* storage can be full or disabled; preferences are non-critical */
  }
}

export function clearPreferences(): void {
  try {
    storage().removeItem(PREFERENCES_KEY);
  } catch {
    /* ignore */
  }
}

/** Raw persisted JSON. Exposed so tests can assert nothing secret is stored. */
export function readStoredPreferences(): string | null {
  try {
    return storage().getItem(PREFERENCES_KEY);
  } catch {
    return null;
  }
}
