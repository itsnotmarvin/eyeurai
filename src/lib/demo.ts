import type { Account, LocalUsageSnapshot, QuotaWindow, Snapshot } from "../types/quota";

/**
 * Deterministic demo snapshot used when the app runs outside Tauri (plain
 * `vite dev`, Storybook-style previews, tests). Given the same `now` it always
 * produces the same output, so screenshots and assertions stay stable while
 * countdowns still tick realistically.
 */

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

interface WindowSeed {
  id: string;
  label: string;
  kind: QuotaWindow["kind"];
  unit: QuotaWindow["unit"];
  used: number;
  limit: number | null;
  /** Offset from `now`, in ms. `null` means the window never resets. */
  resetIn: number | null;
  note?: string;
}

interface AccountSeed {
  id: string;
  provider: Account["provider"];
  label: string;
  plan: string | null;
  source: Account["source"];
  isCliActive?: boolean;
  status: Account["status"];
  message?: string;
  /** How long ago the account was last read, in ms. */
  updatedAgo: number | null;
  windows: WindowSeed[];
}

const ACCOUNT_SEEDS: AccountSeed[] = [
  {
    id: "claude-personal",
    provider: "claude",
    label: "marbin@hey.com",
    plan: "Max 20×",
    source: "oauth",
    isCliActive: true,
    status: "fresh",
    updatedAgo: 42_000,
    windows: [
      {
        id: "session",
        label: "Session",
        kind: "session",
        unit: "tokens",
        used: 118_400,
        limit: 190_000,
        resetIn: 2 * HOUR + 14 * MINUTE,
        note: "5h",
      },
      {
        id: "weekly-all",
        label: "Weekly · all models",
        kind: "weekly",
        unit: "percent",
        used: 43,
        limit: 100,
        resetIn: 3 * DAY + 5 * HOUR,
      },
      {
        id: "weekly-opus",
        label: "Weekly · Opus",
        kind: "weekly",
        unit: "percent",
        used: 78,
        limit: 100,
        resetIn: 3 * DAY + 5 * HOUR,
      },
    ],
  },
  {
    id: "claude-team",
    provider: "claude",
    label: "eng@nimbus.dev",
    plan: "Team · seat 4",
    source: "cli",
    isCliActive: false,
    status: "stale",
    message: "Last read 26 minutes ago — Claude Code was not running.",
    updatedAgo: 26 * MINUTE,
    windows: [
      {
        id: "session",
        label: "Session",
        kind: "session",
        unit: "tokens",
        used: 176_500,
        limit: 190_000,
        resetIn: 38 * MINUTE,
        note: "5h",
      },
      {
        id: "weekly-all",
        label: "Weekly · all models",
        kind: "weekly",
        unit: "percent",
        used: 61,
        limit: 100,
        resetIn: 4 * DAY + 11 * HOUR,
      },
    ],
  },
  {
    id: "openai-pro",
    provider: "openai",
    label: "marbin@hey.com",
    plan: "ChatGPT Pro",
    source: "oauth",
    isCliActive: true,
    status: "fresh",
    updatedAgo: 68_000,
    windows: [
      {
        id: "codex-local",
        label: "Codex · local",
        kind: "session",
        unit: "percent",
        used: 34,
        limit: 100,
        resetIn: 4 * HOUR + 6 * MINUTE,
        note: "5h",
      },
      {
        id: "codex-weekly",
        label: "Codex · weekly",
        kind: "weekly",
        unit: "percent",
        used: 12,
        limit: 100,
        resetIn: 5 * DAY + 2 * HOUR,
      },
    ],
  },
  {
    id: "openai-api",
    provider: "openai",
    label: "nimbus-prod (org)",
    plan: "API · pay as you go",
    source: "api-key",
    status: "error",
    message: "Credentials expired. Re-run `codex login` to reconnect.",
    updatedAgo: 3 * HOUR + 12 * MINUTE,
    windows: [],
  },
  {
    id: "openrouter-main",
    provider: "openrouter",
    label: "nimbus-prod",
    plan: "Credits",
    source: "api-key",
    status: "fresh",
    updatedAgo: 21_000,
    windows: [
      {
        id: "credits",
        label: "Credit balance",
        kind: "credit",
        unit: "usd",
        used: 31.6,
        limit: 50,
        resetIn: null,
        note: "$18.40 left",
      },
      {
        id: "rate",
        label: "Requests · daily",
        kind: "daily",
        unit: "requests",
        used: 1_284,
        limit: 5_000,
        resetIn: 9 * HOUR + 27 * MINUTE,
      },
    ],
  },
  {
    id: "gemini-pro",
    provider: "gemini",
    label: "marbin@gmail.com",
    plan: "Google AI Pro",
    source: "oauth",
    status: "fresh",
    updatedAgo: 95_000,
    windows: [
      {
        id: "pro-daily",
        label: "2.5 Pro · daily",
        kind: "daily",
        unit: "requests",
        used: 91,
        limit: 100,
        resetIn: 13 * HOUR + 3 * MINUTE,
      },
      {
        id: "flash-daily",
        label: "2.5 Flash · daily",
        kind: "daily",
        unit: "requests",
        used: 240,
        limit: 1_500,
        resetIn: 13 * HOUR + 3 * MINUTE,
      },
    ],
  },
];

function buildWindow(seed: WindowSeed, now: number): QuotaWindow {
  const percentUsed =
    seed.limit && seed.limit > 0 ? (seed.used / seed.limit) * 100 : 0;
  return {
    id: seed.id,
    label: seed.label,
    kind: seed.kind,
    unit: seed.unit,
    used: seed.used,
    limit: seed.limit,
    percentUsed: Math.round(percentUsed * 10) / 10,
    resetsAt: seed.resetIn === null ? null : new Date(now + seed.resetIn).toISOString(),
    note: seed.note ?? null,
  };
}

/** Builds the deterministic demo snapshot for a given wall-clock time. */
export function createDemoSnapshot(now: number = Date.now(), recording = false): Snapshot {
  // Anchor to the minute so repeated renders produce identical payloads.
  const anchor = Math.floor(now / MINUTE) * MINUTE;

  return {
    schemaVersion: 1,
    generatedAt: new Date(anchor).toISOString(),
    refreshing: false,
    accounts: ACCOUNT_SEEDS.map((seed) => ({
      id: seed.id,
      provider: seed.provider,
      // Marketing captures must never expose a developer's real address or
      // organisation. The ordinary demo stays unchanged for existing tests.
      label: recording
        ? seed.provider === "openrouter"
          ? "personal key"
          : seed.provider === "gemini"
            ? "personal account"
            : seed.id.endsWith("team") || seed.id.endsWith("api")
              ? "workspace account"
              : "personal account"
        : seed.label,
      plan: seed.plan,
      source: seed.source,
      isCliActive: seed.isCliActive,
      status: seed.status,
      message: seed.message ?? null,
      updatedAt:
        seed.updatedAgo === null
          ? null
          : new Date(anchor - seed.updatedAgo).toISOString(),
      windows: seed.windows.map((window) => buildWindow(window, anchor)),
    })),
  };
}

/**
 * Simulates a refresh in demo mode: usage creeps up slightly and every account
 * reports as freshly read, which is enough to exercise the bar transitions.
 */
export function refreshDemoSnapshot(previous: Snapshot, now: number = Date.now()): Snapshot {
  const base = createDemoSnapshot(now);
  return {
    ...base,
    accounts: base.accounts.map((account, accountIndex) => {
      if (account.status === "error") return account;
      const previousAccount = previous.accounts[accountIndex];
      return {
        ...account,
        label: previousAccount?.label ?? account.label,
        status: "fresh",
        message: null,
        updatedAt: new Date(now).toISOString(),
        windows: account.windows.map((window, windowIndex) => {
          const previousWindow = previousAccount?.windows[windowIndex];
          const drift = ((accountIndex + windowIndex) % 3) + 1;
          const nextPercent = Math.min(
            100,
            (previousWindow?.percentUsed ?? window.percentUsed) + drift * 0.4,
          );
          const limit = window.limit;
          return {
            ...window,
            percentUsed: Math.round(nextPercent * 10) / 10,
            used:
              limit && limit > 0
                ? Math.round(((nextPercent / 100) * limit + Number.EPSILON) * 100) / 100
                : window.used,
          };
        }),
      };
    }),
  };
}

export function createDemoLocalUsage(
  now: number = Date.now(),
  rangeDays: 7 | 30 | 90 = 7,
): LocalUsageSnapshot {
  const scale = rangeDays / 7;
  const scaled = (value: number) => Math.round(value * scale);
  const providerTotals = { claude: scaled(385_000_000), openai: scaled(284_000_000) };
  const daily = (["claude", "openai"] as const).flatMap((provider, providerIndex) => {
    const weights = Array.from({ length: rangeDays }, (_, index) => {
      const wave = 1 + Math.sin((index + providerIndex * 2.3) * 1.17) * 0.55;
      const spike = index % (providerIndex === 0 ? 6 : 8) === 4 ? 2.1 : 1;
      return Math.max(0.1, wave * spike);
    });
    const weightTotal = weights.reduce((sum, weight) => sum + weight, 0);
    return weights.map((weight, index) => ({
      date: new Date(now - (rangeDays - 1 - index) * DAY).toISOString().slice(0, 10),
      provider,
      processedTokens: Math.round((providerTotals[provider] * weight) / weightTotal),
    }));
  });
  return {
    generatedAt: new Date(now).toISOString(),
    rangeDays,
    processedTokens: scaled(669_000_000),
    uncachedInputTokens: scaled(9_500_000),
    cachedInputTokens: scaled(647_000_000),
    cacheWriteInputTokens: scaled(9_900_000),
    outputTokens: scaled(2_600_000),
    reasoningOutputTokens: scaled(445_000),
    observations: scaled(1_248),
    sessions: scaled(47),
    providers: [
      {
        provider: "claude",
        processedTokens: providerTotals.claude,
        uncachedInputTokens: scaled(5_700_000),
        cachedInputTokens: scaled(371_000_000),
        cacheWriteInputTokens: scaled(6_800_000),
        outputTokens: scaled(1_500_000),
        reasoningOutputTokens: 0,
        observations: scaled(729),
        sessions: scaled(28),
      },
      {
        provider: "openai",
        processedTokens: providerTotals.openai,
        uncachedInputTokens: scaled(3_800_000),
        cachedInputTokens: scaled(276_000_000),
        cacheWriteInputTokens: scaled(3_100_000),
        outputTokens: scaled(1_100_000),
        reasoningOutputTokens: scaled(445_000),
        observations: scaled(519),
        sessions: scaled(19),
      },
    ],
    daily,
    models: [
      { provider: "openai", model: "gpt-5.6-sol", processedTokens: scaled(180_000_000) },
      { provider: "claude", model: "claude-fable-5", processedTokens: scaled(220_000_000) },
      { provider: "claude", model: "claude-opus-5", processedTokens: scaled(120_000_000) },
      { provider: "openai", model: "gpt-5.6-terra", processedTokens: scaled(70_000_000) },
      { provider: "claude", model: "claude-sonnet-5", processedTokens: scaled(45_000_000) },
      { provider: "openai", model: "gpt-5.6-luna", processedTokens: scaled(34_000_000) },
    ],
  };
}
