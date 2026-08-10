import type { QuotaKind, QuotaUnit, QuotaWindow, Severity } from "../types/quota";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  if (value < 0) return 0;
  if (value > 100) return 100;
  return value;
}

/** Whole percent for display; never rounds a partially used bar down to 0. */
export function displayPercent(value: number): number {
  const clamped = clampPercent(value);
  if (clamped > 0 && clamped < 1) return 1;
  if (clamped < 100 && clamped > 99) return 99;
  return Math.round(clamped);
}

const MENU_BAR_KIND_LABEL: Record<QuotaKind, string> = {
  session: "Ses",
  daily: "Day",
  weekly: "Wk",
  monthly: "Mo",
  credit: "Cr",
  rate: "Rate",
};

/** Short, stable cadence label for the native menu bar (for example `5h` or `Wk`). */
export function menuBarQuotaLabel(window: QuotaWindow): string {
  const note = window.note?.trim().replace(/\s+/g, "");
  if (note && /^\d+(?:\.\d+)?(?:m|h|d|w)$/i.test(note)) return note;
  const descriptor = `${window.id} ${window.label}`.replace(/[_-]+/g, " ");
  const numericHours = descriptor.match(/\b(\d+(?:\.\d+)?)\s*hours?\b/i);
  if (numericHours?.[1]) return `${numericHours[1]}h`;
  if (/\bfive\s+hours?\b/i.test(descriptor)) return "5h";
  return MENU_BAR_KIND_LABEL[window.kind];
}

export function severityFor(
  percent: number,
  warnThreshold: number,
  criticalThreshold: number,
): Severity {
  const value = clampPercent(percent);
  if (value >= criticalThreshold) return "critical";
  if (value >= warnThreshold) return "warn";
  return "normal";
}

export function parseTimestamp(value: string | null | undefined): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}

/** "2h 14m", "45m", "12d 3h", "now". Always short enough for a bar caption. */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "now";
  if (ms < MINUTE) return `${Math.max(1, Math.round(ms / 1000))}s`;
  if (ms < HOUR) return `${Math.floor(ms / MINUTE)}m`;
  if (ms < DAY) {
    const hours = Math.floor(ms / HOUR);
    const minutes = Math.floor((ms % HOUR) / MINUTE);
    return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`;
  }
  const days = Math.floor(ms / DAY);
  const hours = Math.floor((ms % DAY) / HOUR);
  return hours === 0 ? `${days}d` : `${days}d ${hours}h`;
}

/** Countdown caption for a quota bar, e.g. "resets in 2h 14m". */
export function formatResetCountdown(
  resetsAt: string | null,
  now: number,
): string | null {
  const target = parseTimestamp(resetsAt);
  if (target === null) return null;
  const delta = target - now;
  if (delta <= 0) return "resetting…";
  return `resets in ${formatDuration(delta)}`;
}

/** Relative freshness caption, e.g. "updated 3m ago". */
export function formatRelativeTime(
  timestamp: string | null,
  now: number,
): string {
  const target = parseTimestamp(timestamp);
  if (target === null) return "never updated";
  const delta = now - target;
  if (delta < 45_000) return "updated just now";
  if (delta < 0) return "updated just now";
  return `updated ${formatDuration(delta)} ago`;
}

const compactNumber = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});

const usdFormat = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 2,
});

export function formatQuantity(value: number, unit: QuotaUnit): string {
  if (!Number.isFinite(value)) return "—";
  switch (unit) {
    case "usd":
      return usdFormat.format(value);
    case "percent":
      return `${Math.round(value)}%`;
    case "tokens":
    case "credits":
    case "requests":
    case "messages":
      return value >= 10_000
        ? compactNumber.format(value)
        : new Intl.NumberFormat("en-US").format(Math.round(value * 100) / 100);
    default:
      return String(value);
  }
}

const UNIT_NOUN: Record<QuotaUnit, string> = {
  tokens: "tokens",
  requests: "requests",
  messages: "messages",
  credits: "credits",
  usd: "",
  percent: "",
};

/** "18.2K / 40K tokens", "$4.10 / $25.00", or "62% used" when no limit. */
export function formatUsage(window: QuotaWindow): string {
  const { used, limit, unit } = window;
  if (limit === null || limit <= 0) {
    if (unit === "usd") return `${formatQuantity(used, unit)} used`;
    const noun = UNIT_NOUN[unit];
    return noun
      ? `${formatQuantity(used, unit)} ${noun} used`
      : `${displayPercent(window.percentUsed)}% used`;
  }
  const noun = UNIT_NOUN[unit];
  const base = `${formatQuantity(used, unit)} / ${formatQuantity(limit, unit)}`;
  return noun ? `${base} ${noun}` : base;
}

/** Screen-reader text for a progress bar. */
export function usageAriaText(window: QuotaWindow, now: number): string {
  const parts = [`${displayPercent(window.percentUsed)} percent used`, formatUsage(window)];
  const countdown = formatResetCountdown(window.resetsAt, now);
  if (countdown) parts.push(countdown);
  return parts.join(", ");
}
