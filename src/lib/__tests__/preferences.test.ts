// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";

import {
  DEFAULT_PREFERENCES,
  clearPreferences,
  loadPreferences,
  reconcileThresholds,
  readStoredPreferences,
  sanitizePreferences,
  savePreferences,
} from "../preferences";

describe("sanitizePreferences", () => {
  it("falls back to defaults for junk input", () => {
    expect(sanitizePreferences(null)).toEqual(DEFAULT_PREFERENCES);
    expect(sanitizePreferences("nope")).toEqual(DEFAULT_PREFERENCES);
  });

  it("keeps only known providers, in canonical order", () => {
    const result = sanitizePreferences({
      enabledProviders: ["gemini", "hacker", "claude", "claude"],
    });
    expect(result.enabledProviders).toEqual(["claude", "gemini"]);
  });

  it("allows the full 0–100 range and keeps critical at or above warn", () => {
    const result = sanitizePreferences({ warnThreshold: -5, criticalThreshold: 400 });
    expect(result.warnThreshold).toBe(0);
    expect(result.criticalThreshold).toBe(100);

    const inverted = sanitizePreferences({ warnThreshold: 95, criticalThreshold: 60 });
    expect(inverted.warnThreshold).toBe(60);
    expect(inverted.criticalThreshold).toBe(95);
  });

  it("keeps only valid, unique disconnected account metadata", () => {
    const result = sanitizePreferences({
      disconnectedAccounts: [
        { id: "claude-cli", provider: "claude", label: "Personal" },
        { id: "claude-cli", provider: "claude", label: "Duplicate" },
        { id: "bad", provider: "made-up", label: "Bad" },
      ],
    });
    expect(result.disconnectedAccounts).toEqual([
      { id: "claude-cli", provider: "claude", label: "Personal" },
    ]);
  });

  it("keeps a valid non-secret pinned quota reference", () => {
    expect(
      sanitizePreferences({
        pinnedQuota: { accountId: "  claude-personal ", windowId: " weekly-all  " },
      }).pinnedQuota,
    ).toEqual({ accountId: "claude-personal", windowId: "weekly-all" });
  });

  it("accepts only supported automatic refresh intervals", () => {
    expect(sanitizePreferences({ refreshIntervalSeconds: 15 }).refreshIntervalSeconds).toBe(15);
    expect(sanitizePreferences({ refreshIntervalSeconds: 300 }).refreshIntervalSeconds).toBe(300);
    expect(sanitizePreferences({ refreshIntervalSeconds: 1 }).refreshIntervalSeconds).toBe(60);
    expect(sanitizePreferences({ refreshIntervalSeconds: "15" }).refreshIntervalSeconds).toBe(60);
  });

  it("keeps supported appearance choices and rejects unknown ones", () => {
    const valid = sanitizePreferences({
      appearanceTheme: "lichen",
      backgroundStyle: "photo",
    });
    expect(valid.appearanceTheme).toBe("lichen");
    expect(valid.backgroundStyle).toBe("photo");

    const invalid = sanitizePreferences({
      appearanceTheme: "generic-blue",
      backgroundStyle: "wallpaper-url",
    });
    expect(invalid.appearanceTheme).toBe("porcelain");
    expect(invalid.backgroundStyle).toBe("solid");
  });

  it.each([
    undefined,
    "claude-personal::weekly-all",
    [],
    {},
    { accountId: "", windowId: "weekly-all" },
    { accountId: "claude-personal", windowId: 5 },
    { accountId: "claude\ncutoff", windowId: "weekly-all" },
    { accountId: "a".repeat(161), windowId: "weekly-all" },
  ])("ignores a malformed pinned quota reference: %j", (pinnedQuota) => {
    expect(sanitizePreferences({ pinnedQuota }).pinnedQuota).toBeNull();
  });

  it("migrates the legacy tray display choice to an unpinned quota", () => {
    const result = sanitizePreferences({
      version: 1,
      onboardingCompleted: true,
      enabledProviders: ["openai"],
      notificationsEnabled: true,
      warnThreshold: 64,
      criticalThreshold: 92,
      compact: true,
      disconnectedAccounts: [
        { id: "claude-cli", provider: "claude", label: "Personal" },
      ],
      localUsageEnabled: true,
      trayDisplay: "session-percent",
    });

    expect(result).toMatchObject({
      onboardingCompleted: true,
      enabledProviders: ["openai"],
      notificationsEnabled: true,
      warnThreshold: 64,
      criticalThreshold: 92,
      compact: true,
      disconnectedAccounts: [
        { id: "claude-cli", provider: "claude", label: "Personal" },
      ],
      localUsageEnabled: true,
      pinnedQuota: null,
    });
    expect(result).not.toHaveProperty("trayDisplay");
  });
});

describe("reconcileThresholds", () => {
  it("pushes critical up to warn when warn passes it", () => {
    const next = reconcileThresholds(
      { ...DEFAULT_PREFERENCES, warnThreshold: 92, criticalThreshold: 90 },
      "warn",
    );
    expect(next.warnThreshold).toBe(92);
    expect(next.criticalThreshold).toBe(92);
  });

  it("pulls warn down when critical drops below it", () => {
    const next = reconcileThresholds(
      { ...DEFAULT_PREFERENCES, warnThreshold: 80, criticalThreshold: 60 },
      "critical",
    );
    expect(next.warnThreshold).toBe(60);
    expect(next.criticalThreshold).toBe(60);
  });

  it("produces values that survive sanitisation unchanged", () => {
    const next = reconcileThresholds(
      { ...DEFAULT_PREFERENCES, warnThreshold: 88, criticalThreshold: 70 },
      "critical",
    );
    expect(sanitizePreferences(next)).toEqual(next);
  });
});

describe("storage", () => {
  beforeEach(() => {
    clearPreferences();
  });

  it("round-trips through localStorage", () => {
    savePreferences({
      ...DEFAULT_PREFERENCES,
      onboardingCompleted: true,
      enabledProviders: ["claude"],
      warnThreshold: 60,
      criticalThreshold: 85,
      pinnedQuota: { accountId: "claude-personal", windowId: "weekly-all" },
    });

    const loaded = loadPreferences();
    expect(loaded.onboardingCompleted).toBe(true);
    expect(loaded.enabledProviders).toEqual(["claude"]);
    expect(loaded.warnThreshold).toBe(60);
    expect(loaded.criticalThreshold).toBe(85);
    expect(loaded.pinnedQuota).toEqual({
      accountId: "claude-personal",
      windowId: "weekly-all",
    });
  });

  it("never persists anything secret-looking", () => {
    savePreferences({ ...DEFAULT_PREFERENCES, onboardingCompleted: true });
    const raw = readStoredPreferences() ?? "";
    expect(raw).not.toMatch(/token|secret|key|password|credential/i);
    expect(Object.keys(JSON.parse(raw)).sort()).toEqual([
      "appearanceTheme",
      "backgroundStyle",
      "compact",
      "criticalThreshold",
      "disconnectedAccounts",
      "enabledProviders",
      "localUsageEnabled",
      "notificationsEnabled",
      "onboardingCompleted",
      "pinnedQuota",
      "refreshIntervalSeconds",
      "version",
      "warnThreshold",
    ]);
  });

  it("persists only the stable IDs from a pinned quota reference", () => {
    const preferencesWithExtraPinnedData = {
      ...DEFAULT_PREFERENCES,
      pinnedQuota: {
        accountId: "claude-personal",
        windowId: "weekly-all",
        token: "must-not-leak",
      },
    };
    savePreferences(preferencesWithExtraPinnedData);

    expect(JSON.parse(readStoredPreferences() ?? "{}").pinnedQuota).toEqual({
      accountId: "claude-personal",
      windowId: "weekly-all",
    });
    expect(readStoredPreferences()).not.toContain("must-not-leak");
  });

  it("recovers from corrupted storage", () => {
    const store = window.localStorage;
    if (typeof store?.setItem === "function") {
      store.setItem("eyeurai.preferences.v1", "{not json");
      expect(loadPreferences()).toEqual(DEFAULT_PREFERENCES);
    }
  });

  it("clears cleanly", () => {
    savePreferences({ ...DEFAULT_PREFERENCES, onboardingCompleted: true });
    expect(readStoredPreferences()).not.toBeNull();
    clearPreferences();
    expect(readStoredPreferences()).toBeNull();
    expect(loadPreferences()).toEqual(DEFAULT_PREFERENCES);
  });
});
