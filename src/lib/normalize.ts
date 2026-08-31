import {
  PROVIDER_IDS,
  type Account,
  type AccountStatus,
  type AuthSource,
  type ProviderId,
  type QuotaKind,
  type QuotaUnit,
  type QuotaWindow,
  type RemediationChoice,
  type RemediationChoiceKind,
  type RemediationImpact,
  type RemediationPlan,
  type Snapshot,
} from "../types/quota";
import { clampPercent } from "./format";

/**
 * Tolerant parser for the backend snapshot.
 *
 * The Rust side may serialise with either `camelCase` or the serde default
 * `snake_case`; both are accepted here so the UI never breaks on a rename.
 * Anything unrecognised is coerced to a safe default rather than thrown away,
 * because a partially readable snapshot is far more useful than an error card.
 */

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Reads `key` in camelCase or snake_case form. */
function pick(source: Json, key: string): unknown {
  if (key in source) return source[key];
  const snake = key.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
  if (snake in source) return source[snake];
  const kebab = key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
  return kebab in source ? source[kebab] : undefined;
}

function asString(value: unknown, fallback = ""): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  return fallback;
}

function asOptionalString(value: unknown): string | null {
  const text = typeof value === "string" ? value.trim() : "";
  return text.length > 0 ? text : null;
}

function asNumber(value: unknown, fallback = 0): number {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Number.parseFloat(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return fallback;
}

function asOptionalNumber(value: unknown): number | null {
  if (value === null || value === undefined || value === "") return null;
  const parsed = asNumber(value, Number.NaN);
  return Number.isFinite(parsed) ? parsed : null;
}

function asMeasureValue(value: unknown): number | null {
  if (isObject(value)) return asOptionalNumber(pick(value, "value"));
  return asOptionalNumber(value);
}

function asErrorMessage(value: unknown, includeLegacyRemediation = true): string | null {
  if (typeof value === "string") return asOptionalString(value);
  if (!isObject(value)) return null;
  const message = asOptionalString(pick(value, "message"));
  const remediation = asOptionalString(pick(value, "remediation"));
  if (includeLegacyRemediation && message && remediation) return `${message}. ${remediation}`;
  return message ?? remediation;
}

function asBoolean(value: unknown, fallback = false): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function asOptionalBoolean(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

function normalizeEnum<T extends string>(
  value: unknown,
  allowed: readonly T[],
  fallback: T,
): T {
  if (typeof value !== "string") return fallback;
  const needle = value.trim().toLowerCase().replace(/[\s_]+/g, "-");
  for (const option of allowed) {
    if (option === needle) return option;
  }
  return fallback;
}

const PROVIDER_ALIASES: Record<string, ProviderId> = {
  claude: "claude",
  anthropic: "claude",
  "claude-code": "claude",
  openai: "openai",
  codex: "openai",
  chatgpt: "openai",
  "openai-codex": "openai",
  openrouter: "openrouter",
  "open-router": "openrouter",
  gemini: "gemini",
  google: "gemini",
  "google-gemini": "gemini",
};

export function normalizeProvider(value: unknown): ProviderId | null {
  if (typeof value !== "string") return null;
  const needle = value.trim().toLowerCase().replace(/[\s_]+/g, "-");
  const direct = PROVIDER_ALIASES[needle];
  if (direct) return direct;
  return PROVIDER_IDS.find((id) => needle.startsWith(id)) ?? null;
}

const UNITS: readonly QuotaUnit[] = [
  "tokens",
  "requests",
  "messages",
  "credits",
  "usd",
  "percent",
];

const KINDS: readonly QuotaKind[] = [
  "session",
  "daily",
  "weekly",
  "monthly",
  "credit",
  "rate",
];

const SOURCES: readonly AuthSource[] = ["oauth", "api-key", "cli", "unknown"];

function normalizeIsoDate(value: unknown): string | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    // Accept unix seconds or milliseconds.
    const ms = value > 1e11 ? value : value * 1000;
    return new Date(ms).toISOString();
  }
  const text = asOptionalString(value);
  if (!text) return null;
  return Number.isNaN(Date.parse(text)) ? null : text;
}

function normalizeWindow(raw: unknown, index: number): QuotaWindow | null {
  if (!isObject(raw)) return null;
  const rawUsed = pick(raw, "used") ?? pick(raw, "consumed");
  const rawLimit = pick(raw, "limit") ?? pick(raw, "total");
  const limit = asMeasureValue(rawLimit);
  const used = asMeasureValue(rawUsed) ?? 0;
  const rawPercent = pick(raw, "percentUsed") ?? pick(raw, "percent") ?? pick(raw, "usedPercent");
  const percentUsed =
    rawPercent === undefined || rawPercent === null
      ? limit && limit > 0
        ? (used / limit) * 100
        : 0
      : asNumber(rawPercent, 0);

  const rawKind = asString(pick(raw, "kind")).toLowerCase().replace(/[\s_]+/g, "-");
  const key = asString(pick(raw, "id") ?? pick(raw, "key"), `window-${index}`);
  const label = asString(pick(raw, "label") ?? pick(raw, "name"), "Usage");
  const cadenceText = `${key} ${label}`.toLowerCase();
  let kind = normalizeEnum(pick(raw, "kind"), KINDS, "session");
  if (rawKind === "rolling") kind = cadenceText.includes("week") ? "weekly" : "session";
  if (rawKind === "calendar") {
    kind = cadenceText.includes("week")
      ? "weekly"
      : cadenceText.includes("day")
        ? "daily"
        : "monthly";
  }
  if (rawKind === "balance" || rawKind === "allowance") kind = "credit";
  if (rawKind === "rate-limit") kind = "rate";

  const measure = isObject(rawUsed) ? rawUsed : isObject(rawLimit) ? rawLimit : null;
  const rawUnit = pick(raw, "unit") ?? (measure ? pick(measure, "unit") : undefined);

  return {
    id: key,
    label,
    kind,
    unit: normalizeEnum(rawUnit, UNITS, "percent"),
    used,
    limit,
    percentUsed: clampPercent(percentUsed),
    resetsAt: normalizeIsoDate(pick(raw, "resetsAt") ?? pick(raw, "resetAt")),
    note: asOptionalString(pick(raw, "note")),
  };
}

function defaultSource(provider: ProviderId): AuthSource {
  if (provider === "claude") return "oauth";
  if (provider === "openai") return "cli";
  if (provider === "openrouter" || provider === "gemini") return "api-key";
  return "unknown";
}

function normalizeStatus(raw: unknown, stale: boolean): AccountStatus {
  if (stale) return "stale";
  // Preserve legacy payloads that predate an explicit status field, but fail
  // closed when a present value is malformed or comes from a newer backend we
  // do not understand. Unknown state must never contribute to a live peak.
  if (raw === undefined) return "fresh";
  if (typeof raw !== "string") return "stale";
  const value = raw.trim().toLowerCase().replace(/[\s-]+/g, "_");
  if (value === "fresh" || value === "ok") return "fresh";
  if (value === "pending") return "pending";
  if (value === "stale" || value === "partial" || value === "rate_limited") return "stale";
  if (value === "not_configured") return "unconfigured";
  if (value === "unsupported") return "unsupported";
  if (
    value === "error" ||
    value === "unauthorized"
  ) {
    return "error";
  }
  return "stale";
}

const REMEDIATION_KINDS: readonly RemediationChoiceKind[] = [
  "managed-login",
  "open-terminal",
  "retry",
  "open-settings",
];
const REMEDIATION_IMPACTS: readonly RemediationImpact[] = [
  "app-only",
  "global-cli-identity",
  "read-only",
];

function normalizeRemediationChoice(raw: unknown): RemediationChoice | null {
  if (!isObject(raw)) return null;
  const id = asOptionalString(pick(raw, "choiceId") ?? pick(raw, "id"));
  const label = asOptionalString(pick(raw, "label"));
  if (!id || !label) return null;
  return {
    id,
    kind: normalizeEnum(pick(raw, "kind"), REMEDIATION_KINDS, "retry"),
    label,
    detail: asOptionalString(pick(raw, "detail")),
    commandPreview: asOptionalString(pick(raw, "commandPreview")),
    impact: normalizeEnum(pick(raw, "impact"), REMEDIATION_IMPACTS, "read-only"),
  };
}

function normalizeRemediationPlan(raw: unknown): RemediationPlan | null {
  if (!isObject(raw)) return null;
  const id = asOptionalString(pick(raw, "planId") ?? pick(raw, "id"));
  const title = asOptionalString(pick(raw, "title"));
  const rawChoices = pick(raw, "choices");
  if (!id || !title || !Array.isArray(rawChoices)) return null;
  const choices = rawChoices
    .map(normalizeRemediationChoice)
    .filter((choice): choice is RemediationChoice => choice !== null);
  if (choices.length === 0) return null;
  return {
    id,
    title,
    detail: asString(pick(raw, "detail")),
    choices,
  };
}

function normalizeAccount(
  raw: unknown,
  index: number,
  inheritedProvider?: ProviderId,
  inheritedError?: unknown,
): Account | null {
  if (!isObject(raw)) return null;
  const provider = normalizeProvider(pick(raw, "provider")) ?? inheritedProvider ?? null;
  if (!provider) return null;

  const rawWindows = pick(raw, "windows") ?? pick(raw, "quotas");
  const windows = Array.isArray(rawWindows)
    ? rawWindows
        .map((entry, windowIndex) => normalizeWindow(entry, windowIndex))
        .filter((entry): entry is QuotaWindow => entry !== null)
    : [];

  const freshness = isObject(pick(raw, "freshness")) ? (pick(raw, "freshness") as Json) : null;
  const status = normalizeStatus(pick(raw, "status"), asBoolean(freshness?.stale, false));
  const ownError = pick(raw, "error");
  const rawSource = pick(raw, "source");
  const remediation = normalizeRemediationPlan(pick(raw, "remediationPlan"));

  return {
    id: asString(pick(raw, "id") ?? pick(raw, "accountId"), `${provider}-${index}`),
    provider,
    label: asString(pick(raw, "label") ?? pick(raw, "name"), "Account"),
    plan: asOptionalString(pick(raw, "plan")),
    source:
      rawSource === undefined
        ? defaultSource(provider)
        : normalizeEnum(rawSource, SOURCES, "unknown"),
    isCliActive: asOptionalBoolean(pick(raw, "isCliActive") ?? pick(raw, "active")),
    status,
    message:
      asOptionalString(pick(raw, "message")) ??
      asErrorMessage(ownError, !remediation) ??
      asErrorMessage(inheritedError, !remediation),
    remediation,
    updatedAt: normalizeIsoDate(
      pick(raw, "updatedAt") ??
        pick(raw, "lastUpdated") ??
        (freshness ? pick(freshness, "fetchedAt") : undefined),
    ),
    windows,
  };
}

function normalizeProviderSnapshot(raw: unknown, providerIndex: number): Account[] {
  if (!isObject(raw)) return [];
  const provider = normalizeProvider(pick(raw, "provider"));
  if (!provider) return [];
  const providerError = pick(raw, "error");
  const rawAccounts = pick(raw, "accounts");
  if (Array.isArray(rawAccounts) && rawAccounts.length > 0) {
    return rawAccounts
      .map((entry, index) => normalizeAccount(entry, index, provider, providerError))
      .filter((entry): entry is Account => entry !== null);
  }

  const displayName = asString(pick(raw, "displayName"), PROVIDER_META_NAME[provider]);
  const freshness = isObject(pick(raw, "freshness")) ? (pick(raw, "freshness") as Json) : null;
  const remediation = normalizeRemediationPlan(pick(raw, "remediationPlan"));
  return [
    {
      id: `${provider}-status-${providerIndex}`,
      provider,
      label: displayName,
      plan: null,
      source: defaultSource(provider),
      status: normalizeStatus(pick(raw, "status"), asBoolean(freshness?.stale, false)),
      message: asErrorMessage(providerError, !remediation),
      remediation,
      updatedAt: normalizeIsoDate(freshness ? pick(freshness, "fetchedAt") : undefined),
      windows: [],
    },
  ];
}

const PROVIDER_META_NAME: Record<ProviderId, string> = {
  claude: "Claude",
  openai: "Codex / ChatGPT",
  openrouter: "OpenRouter",
  gemini: "Gemini",
};

export function normalizeSnapshot(raw: unknown): Snapshot | null {
  if (!isObject(raw)) return null;
  const rawAccounts = pick(raw, "accounts");
  const rawProviders = pick(raw, "providers");
  if (!Array.isArray(rawAccounts) && !Array.isArray(rawProviders)) return null;

  const accounts = Array.isArray(rawAccounts)
    ? rawAccounts
        .map((entry, index) => normalizeAccount(entry, index))
        .filter((entry): entry is Account => entry !== null)
    : (rawProviders as unknown[]).flatMap((entry, index) =>
        normalizeProviderSnapshot(entry, index),
      );

  return {
    schemaVersion: Math.trunc(asNumber(pick(raw, "schemaVersion"), 1)),
    generatedAt: normalizeIsoDate(pick(raw, "generatedAt")) ?? new Date().toISOString(),
    refreshing: asBoolean(pick(raw, "refreshing"), false),
    accounts,
  };
}
