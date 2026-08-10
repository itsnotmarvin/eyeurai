import { describe, expect, it } from "vitest";

import { PROVIDER_IDS } from "../../types/quota";
import { createDemoSnapshot, refreshDemoSnapshot } from "../demo";

const NOW = Date.parse("2025-03-04T12:00:30.000Z");

describe("createDemoSnapshot", () => {
  it("is deterministic for a given clock", () => {
    expect(createDemoSnapshot(NOW)).toEqual(createDemoSnapshot(NOW));
  });

  it("anchors timestamps to the minute so renders are stable", () => {
    const snapshot = createDemoSnapshot(NOW);
    expect(snapshot.generatedAt).toBe("2025-03-04T12:00:00.000Z");
  });

  it("covers every provider, with multiple accounts for some", () => {
    const snapshot = createDemoSnapshot(NOW);
    const providers = new Set(snapshot.accounts.map((account) => account.provider));
    for (const provider of PROVIDER_IDS) {
      expect(providers.has(provider)).toBe(true);
    }
    expect(snapshot.accounts.filter((account) => account.provider === "claude").length).toBe(2);
  });

  it("exercises fresh, stale and error states", () => {
    const statuses = new Set(createDemoSnapshot(NOW).accounts.map((account) => account.status));
    expect(statuses).toEqual(new Set(["fresh", "stale", "error"]));
  });

  it("produces percentages inside 0–100 that match used/limit", () => {
    for (const account of createDemoSnapshot(NOW).accounts) {
      for (const window of account.windows) {
        expect(window.percentUsed).toBeGreaterThanOrEqual(0);
        expect(window.percentUsed).toBeLessThanOrEqual(100);
        if (window.limit) {
          expect(window.percentUsed).toBeCloseTo((window.used / window.limit) * 100, 1);
        }
      }
    }
  });
});

describe("refreshDemoSnapshot", () => {
  it("nudges usage upward without exceeding 100%", () => {
    const first = createDemoSnapshot(NOW);
    const second = refreshDemoSnapshot(first, NOW + 1000);

    const before = first.accounts[0]?.windows[0]?.percentUsed ?? 0;
    const after = second.accounts[0]?.windows[0]?.percentUsed ?? 0;
    expect(after).toBeGreaterThan(before);

    for (const account of second.accounts) {
      for (const window of account.windows) {
        expect(window.percentUsed).toBeLessThanOrEqual(100);
      }
    }
  });

  it("keeps failing accounts failing", () => {
    const first = createDemoSnapshot(NOW);
    const second = refreshDemoSnapshot(first, NOW + 1000);
    const errored = second.accounts.find((account) => account.id === "openai-api");
    expect(errored?.status).toBe("error");
  });
});
