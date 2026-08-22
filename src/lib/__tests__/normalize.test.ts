import { describe, expect, it } from "vitest";

import { normalizeProvider, normalizeSnapshot } from "../normalize";

describe("normalizeProvider", () => {
  it("maps backend aliases onto the four supported providers", () => {
    expect(normalizeProvider("anthropic")).toBe("claude");
    expect(normalizeProvider("Claude_Code")).toBe("claude");
    expect(normalizeProvider("codex")).toBe("openai");
    expect(normalizeProvider("open-router")).toBe("openrouter");
    expect(normalizeProvider("Google")).toBe("gemini");
    expect(normalizeProvider("mistral")).toBeNull();
  });
});

describe("normalizeSnapshot", () => {
  it("accepts a serde snake_case payload", () => {
    const snapshot = normalizeSnapshot({
      schema_version: 2,
      generated_at: "2025-03-04T12:00:00Z",
      refreshing: true,
      accounts: [
        {
          id: "acc-1",
          provider: "anthropic",
          label: "work@acme.com",
          plan: "Max 20x",
          source: "oauth",
          status: "fresh",
          last_updated: "2025-03-04T11:59:00Z",
          quotas: [
            {
              id: "session",
              label: "Session",
              kind: "session",
              unit: "tokens",
              used: 50,
              limit: 200,
              resets_at: "2025-03-04T15:00:00Z",
            },
          ],
        },
      ],
    });

    expect(snapshot).not.toBeNull();
    expect(snapshot?.schemaVersion).toBe(2);
    expect(snapshot?.refreshing).toBe(true);
    const account = snapshot?.accounts[0];
    expect(account?.provider).toBe("claude");
    expect(account?.updatedAt).toBe("2025-03-04T11:59:00Z");
    expect(account?.windows[0]?.resetsAt).toBe("2025-03-04T15:00:00Z");
  });

  it("derives percentUsed when the backend omits it", () => {
    const snapshot = normalizeSnapshot({
      accounts: [
        {
          provider: "openai",
          windows: [{ id: "w", used: 25, limit: 200 }],
        },
      ],
    });
    expect(snapshot?.accounts[0]?.windows[0]?.percentUsed).toBe(12.5);
  });

  it("clamps percentages and defaults unknown enums", () => {
    const snapshot = normalizeSnapshot({
      accounts: [
        {
          provider: "gemini",
          status: "WEIRD",
          source: "magic",
          windows: [{ id: "w", percentUsed: 480, unit: "bananas", kind: "eon" }],
        },
      ],
    });
    const account = snapshot?.accounts[0];
    expect(account?.status).toBe("fresh");
    expect(account?.source).toBe("unknown");
    expect(account?.windows[0]?.percentUsed).toBe(100);
    expect(account?.windows[0]?.unit).toBe("percent");
    expect(account?.windows[0]?.kind).toBe("session");
  });

  it("drops accounts with unknown providers but keeps the rest", () => {
    const snapshot = normalizeSnapshot({
      accounts: [{ provider: "cohere" }, { provider: "openrouter", label: "prod" }],
    });
    expect(snapshot?.accounts).toHaveLength(1);
    expect(snapshot?.accounts[0]?.label).toBe("prod");
  });

  it("flattens the normalized Rust provider snapshot", () => {
    const snapshot = normalizeSnapshot({
      schema_version: 1,
      generated_at: "2026-08-09T18:00:00Z",
      source: "live",
      providers: [
        {
          provider: "codex",
          display_name: "Codex / ChatGPT",
          status: "ok",
          accounts: [
            {
              account_id: "codex-cli",
              label: "dev@example.com",
              plan: "Plus",
              active: true,
              status: "ok",
              freshness: { source: "live", fetched_at: "2026-08-09T17:59:58Z", stale: false },
              windows: [
                {
                  key: "five_hour",
                  label: "5-hour session",
                  kind: "rolling",
                  used_percent: 63.5,
                  used: { value: 63.5, unit: "percent" },
                  limit: { value: 100, unit: "percent" },
                  resets_at: "2026-08-09T20:00:00Z",
                },
              ],
            },
          ],
          freshness: { source: "live", fetched_at: "2026-08-09T17:59:58Z", stale: false },
        },
      ],
    });

    expect(snapshot?.accounts).toHaveLength(1);
    expect(snapshot?.accounts[0]).toMatchObject({
      id: "codex-cli",
      provider: "openai",
      source: "cli",
      isCliActive: true,
      status: "fresh",
      updatedAt: "2026-08-09T17:59:58Z",
    });
    expect(snapshot?.accounts[0]?.windows[0]).toMatchObject({
      id: "five_hour",
      kind: "session",
      unit: "percent",
      used: 63.5,
      limit: 100,
      percentUsed: 63.5,
    });
  });

  it("normalizes CLI-active aliases while preserving legacy absence", () => {
    const snapshot = normalizeSnapshot({
      accounts: [
        { id: "retained", provider: "openai", active: false },
        { id: "current", provider: "claude", isCliActive: true },
        { id: "legacy", provider: "openai" },
      ],
    });

    expect(snapshot?.accounts.map((account) => account.isCliActive)).toEqual([
      false,
      true,
      undefined,
    ]);
  });

  it("preserves backend-authored remediation plans without flattening their command", () => {
    const snapshot = normalizeSnapshot({
      providers: [
        {
          provider: "claude",
          accounts: [
            {
              account_id: "claude:principal",
              label: "work@example.com",
              active: false,
              status: "not_configured",
              error: {
                message: "This account is not the current Claude Code login.",
                remediation: "Run `claude /login`.",
              },
              remediation_plan: {
                plan_id: "opaque-plan",
                title: "Reconnect this Claude account?",
                detail: "Choose a safe path.",
                choices: [
                  {
                    choice_id: "managed-login",
                    kind: "managed_login",
                    label: "Reconnect inside EyeUrAI",
                    impact: "app_only",
                  },
                  {
                    choice_id: "open-terminal",
                    kind: "open_terminal",
                    label: "Switch Claude Code account…",
                    command_preview: "claude /login",
                    impact: "global_cli_identity",
                  },
                ],
              },
              freshness: { source: "cache", stale: true },
              windows: [],
            },
          ],
        },
      ],
    });

    expect(snapshot?.accounts[0]?.message).toBe(
      "This account is not the current Claude Code login.",
    );
    expect(snapshot?.accounts[0]?.remediation).toMatchObject({
      id: "opaque-plan",
      choices: [
        { id: "managed-login", kind: "managed-login", impact: "app-only" },
        {
          id: "open-terminal",
          kind: "open-terminal",
          commandPreview: "claude /login",
          impact: "global-cli-identity",
        },
      ],
    });
  });

  it("turns a provider-level unsupported state into a visible card", () => {
    const snapshot = normalizeSnapshot({
      providers: [
        {
          provider: "gemini",
          display_name: "Gemini",
          status: "unsupported",
          accounts: [],
          error: {
            message: "No supported usage-percentage API",
            remediation: "Open Google AI Studio.",
          },
          freshness: { source: "none", stale: false },
        },
      ],
    });
    expect(snapshot?.accounts[0]).toMatchObject({
      provider: "gemini",
      status: "unsupported",
      label: "Gemini",
    });
    expect(snapshot?.accounts[0]?.message).toContain("Open Google AI Studio");
  });

  it("rejects payloads that are not snapshots", () => {
    expect(normalizeSnapshot(null)).toBeNull();
    expect(normalizeSnapshot("nope")).toBeNull();
    expect(normalizeSnapshot({ accounts: "nope" })).toBeNull();
  });
});
