import { describe, expect, it } from "vitest";

import type { QuotaWindow } from "../../types/quota";
import {
  displayPercent,
  formatDuration,
  formatRelativeTime,
  formatResetCountdown,
  formatUsage,
  menuBarQuotaLabel,
  menuBarResetCountdown,
  severityFor,
  usageAriaText,
} from "../format";

const NOW = Date.parse("2025-03-04T12:00:00.000Z");

function makeWindow(overrides: Partial<QuotaWindow> = {}): QuotaWindow {
  return {
    id: "session",
    label: "Session",
    kind: "session",
    unit: "tokens",
    used: 118_400,
    limit: 190_000,
    percentUsed: 62.3,
    resetsAt: new Date(NOW + 2 * 3_600_000 + 14 * 60_000).toISOString(),
    note: null,
    ...overrides,
  };
}

describe("displayPercent", () => {
  it("rounds to whole percents", () => {
    expect(displayPercent(62.3)).toBe(62);
    expect(displayPercent(62.6)).toBe(63);
  });

  it("never hides a barely-used or barely-complete bar", () => {
    expect(displayPercent(0.2)).toBe(1);
    expect(displayPercent(99.6)).toBe(99);
    expect(displayPercent(100)).toBe(100);
    expect(displayPercent(0)).toBe(0);
  });

  it("clamps out-of-range and non-finite input", () => {
    expect(displayPercent(140)).toBe(100);
    expect(displayPercent(-12)).toBe(0);
    expect(displayPercent(Number.NaN)).toBe(0);
  });
});

describe("menuBarQuotaLabel", () => {
  it("prefers concise time-window notes", () => {
    expect(menuBarQuotaLabel(makeWindow({ note: "5h" }))).toBe("5h");
    expect(menuBarQuotaLabel(makeWindow({ note: " 7 d " }))).toBe("7d");
  });

  it("recognises live Claude and Codex five-hour windows without a note", () => {
    expect(
      menuBarQuotaLabel(
        makeWindow({ id: "five_hour", label: "5-hour session", note: null }),
      ),
    ).toBe("5h");
    expect(
      menuBarQuotaLabel(makeWindow({ id: "window_7200s", label: "2 hours", note: null })),
    ).toBe("2h");
  });

  it("uses stable cadence abbreviations for other quota windows", () => {
    expect(menuBarQuotaLabel(makeWindow({ kind: "weekly" }))).toBe("Wk");
    expect(menuBarQuotaLabel(makeWindow({ kind: "daily" }))).toBe("Day");
    expect(menuBarQuotaLabel(makeWindow({ kind: "monthly" }))).toBe("Mo");
    expect(menuBarQuotaLabel(makeWindow({ kind: "rate" }))).toBe("Rate");
    expect(menuBarQuotaLabel(makeWindow({ kind: "credit", unit: "usd" }))).toBe("Cr");
  });

  it("does not put verbose provider notes in the menu bar", () => {
    expect(menuBarQuotaLabel(makeWindow({ kind: "credit", note: "$18.40 left" }))).toBe("Cr");
  });
});

describe("menuBarResetCountdown", () => {
  it("formats a live reset value without the sentence prefix", () => {
    expect(menuBarResetCountdown(new Date(NOW + 10 * 60_000).toISOString(), NOW)).toBe("10m");
    expect(menuBarResetCountdown(new Date(NOW - 1).toISOString(), NOW)).toBe("now");
    expect(menuBarResetCountdown(null, NOW)).toBeNull();
  });
});

describe("severityFor", () => {
  it("uses the user thresholds inclusively", () => {
    expect(severityFor(50, 75, 90)).toBe("normal");
    expect(severityFor(75, 75, 90)).toBe("warn");
    expect(severityFor(89.9, 75, 90)).toBe("warn");
    expect(severityFor(90, 75, 90)).toBe("critical");
    expect(severityFor(100, 75, 90)).toBe("critical");
  });
});

describe("formatDuration", () => {
  it("renders compact, human units", () => {
    expect(formatDuration(45_000)).toBe("45s");
    expect(formatDuration(9 * 60_000)).toBe("9m");
    expect(formatDuration(2 * 3_600_000 + 14 * 60_000)).toBe("2h 14m");
    expect(formatDuration(5 * 3_600_000)).toBe("5h");
    expect(formatDuration(3 * 86_400_000 + 5 * 3_600_000)).toBe("3d 5h");
    expect(formatDuration(0)).toBe("now");
    expect(formatDuration(-1)).toBe("now");
  });
});

describe("formatResetCountdown", () => {
  it("builds a reset caption", () => {
    expect(formatResetCountdown(makeWindow().resetsAt, NOW)).toBe("resets in 2h 14m");
  });

  it("returns null when a window never resets", () => {
    expect(formatResetCountdown(null, NOW)).toBeNull();
  });

  it("shows a resetting state once the deadline passes", () => {
    expect(formatResetCountdown(new Date(NOW - 1000).toISOString(), NOW)).toBe("resetting…");
  });

  it("ignores unparsable timestamps", () => {
    expect(formatResetCountdown("not-a-date", NOW)).toBeNull();
  });
});

describe("formatRelativeTime", () => {
  it("describes freshness", () => {
    expect(formatRelativeTime(new Date(NOW - 20_000).toISOString(), NOW)).toBe("updated just now");
    expect(formatRelativeTime(new Date(NOW - 5 * 60_000).toISOString(), NOW)).toBe("updated 5m ago");
    expect(formatRelativeTime(null, NOW)).toBe("never updated");
  });
});

describe("formatUsage", () => {
  it("compacts large token counts", () => {
    expect(formatUsage(makeWindow())).toBe("118.4K / 190K tokens");
  });

  it("formats currency windows", () => {
    expect(
      formatUsage(makeWindow({ unit: "usd", used: 31.6, limit: 50, percentUsed: 63.2 })),
    ).toBe("$31.60 / $50.00");
  });

  it("falls back to a percentage when there is no limit", () => {
    expect(
      formatUsage(makeWindow({ unit: "percent", used: 40, limit: null, percentUsed: 40 })),
    ).toBe("40% used");
  });
});

describe("usageAriaText", () => {
  it("reads percentage, absolute usage and reset time", () => {
    expect(usageAriaText(makeWindow(), NOW)).toBe(
      "62 percent used, 118.4K / 190K tokens, resets in 2h 14m",
    );
  });
});
