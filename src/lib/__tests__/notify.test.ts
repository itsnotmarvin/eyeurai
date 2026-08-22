import { describe, expect, it } from "vitest";

import type { Preferences, Snapshot } from "../../types/quota";
import { DEFAULT_PREFERENCES } from "../preferences";
import { computeAlerts, type AlertState } from "../notify";

const preferences: Preferences = {
  ...DEFAULT_PREFERENCES,
  onboardingCompleted: true,
  notificationsEnabled: true,
  warnThreshold: 75,
  criticalThreshold: 90,
};

function snapshotWith(percent: number, overrides: Partial<Snapshot["accounts"][number]> = {}): Snapshot {
  return {
    schemaVersion: 1,
    generatedAt: "2025-03-04T12:00:00.000Z",
    refreshing: false,
    accounts: [
      {
        id: "claude-1",
        provider: "claude",
        label: "work@acme.com",
        plan: "Max 20×",
        source: "oauth",
        status: "fresh",
        message: null,
        updatedAt: "2025-03-04T12:00:00.000Z",
        windows: [
          {
            id: "session",
            label: "Session",
            kind: "session",
            unit: "percent",
            used: percent,
            limit: 100,
            percentUsed: percent,
            resetsAt: null,
            note: null,
          },
        ],
        ...overrides,
      },
    ],
  };
}

describe("computeAlerts", () => {
  it("stays quiet below the warn threshold", () => {
    const { alerts, state } = computeAlerts(snapshotWith(40), preferences, {});
    expect(alerts).toHaveLength(0);
    expect(state["claude-1::session"]).toBe("normal");
  });

  it("fires once when a bar crosses into warn", () => {
    const first = computeAlerts(snapshotWith(78), preferences, {});
    expect(first.alerts).toHaveLength(1);
    expect(first.alerts[0]?.severity).toBe("warn");
    expect(first.alerts[0]?.body).toBe("work@acme.com · Session at 78%");

    const second = computeAlerts(snapshotWith(80), preferences, first.state);
    expect(second.alerts).toHaveLength(0);
  });

  it("escalates from warn to critical", () => {
    const warned: AlertState = { "claude-1::session": "warn" };
    const { alerts } = computeAlerts(snapshotWith(93), preferences, warned);
    expect(alerts).toHaveLength(1);
    expect(alerts[0]?.severity).toBe("critical");
    expect(alerts[0]?.title).toBe("Claude limit almost gone");
  });

  it("re-arms after the window resets", () => {
    const critical: AlertState = { "claude-1::session": "critical" };
    const afterReset = computeAlerts(snapshotWith(3), preferences, critical);
    expect(afterReset.alerts).toHaveLength(0);
    expect(afterReset.state["claude-1::session"]).toBe("normal");

    const again = computeAlerts(snapshotWith(96), preferences, afterReset.state);
    expect(again.alerts).toHaveLength(1);
  });

  it("ignores hidden providers and every non-live account state", () => {
    const hidden = computeAlerts(snapshotWith(99), {
      ...preferences,
      enabledProviders: ["gemini"],
    }, {});
    expect(hidden.alerts).toHaveLength(0);

    const errored = computeAlerts(snapshotWith(99, { status: "error" }), preferences, {});
    expect(errored.alerts).toHaveLength(0);

    for (const status of ["stale", "pending", "unconfigured", "unsupported"] as const) {
      const result = computeAlerts(snapshotWith(99, { status }), preferences, {});
      expect(result.alerts).toHaveLength(0);
      expect(result.state).not.toHaveProperty("claude-1::session");
    }
  });
});
