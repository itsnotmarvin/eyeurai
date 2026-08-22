// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "../App";
import * as demo from "../lib/demo";
import * as ipc from "../lib/ipc";
import {
  DEFAULT_PREFERENCES,
  clearPreferences,
  loadPreferences,
  savePreferences,
} from "../lib/preferences";

function seedPreferences(overrides: Partial<typeof DEFAULT_PREFERENCES> = {}): void {
  savePreferences({ ...DEFAULT_PREFERENCES, onboardingCompleted: true, ...overrides });
}

beforeEach(() => {
  clearPreferences();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("App", () => {
  it("runs onboarding on first launch and remembers the outcome", async () => {
    render(<App />);

    expect(screen.getByText("Which limits should we watch?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));

    expect(await screen.findByRole("heading", { name: "Claude" })).toBeInTheDocument();
    expect(loadPreferences().onboardingCompleted).toBe(true);
  });

  it("renders every connected account, grouped per provider", async () => {
    seedPreferences();
    render(<App />);

    await screen.findByRole("heading", { name: "Claude" });

    const claude = screen.getByRole("region", { name: "Claude" });
    expect(within(claude).getAllByRole("article")).toHaveLength(2);
    expect(within(claude).getByText("2 accounts")).toBeInTheDocument();

    for (const provider of ["Claude", "OpenAI", "OpenRouter", "Gemini"]) {
      expect(screen.getByRole("heading", { name: provider })).toBeInTheDocument();
    }
    expect(screen.getAllByRole("article").length).toBeGreaterThanOrEqual(6);
    expect(screen.getAllByRole("progressbar").length).toBeGreaterThan(6);

    const retainedAccount = screen.getByRole("article", {
      name: "Claude · eng@nimbus.dev",
    });
    expect(within(retainedAccount).getByText("Last known")).toBeInTheDocument();
    expect(within(retainedAccount).queryByText("Live")).not.toBeInTheDocument();
  });

  it("labels demo data so preview mode is never mistaken for live data", async () => {
    seedPreferences();
    render(<App />);
    expect(await screen.findByText("demo")).toBeInTheDocument();
  });

  it("opens and closes settings with the keyboard", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.keyDown(window, { key: "," });
    expect(await screen.findByRole("region", { name: "Settings" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("region", { name: "Settings" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("heading", { name: "Claude" })).toBeInTheDocument();
  });

  it("hides a provider turned off in settings and persists it", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    const settings = await screen.findByRole("region", { name: "Settings" });
    fireEvent.click(within(settings).getByRole("switch", { name: "Claude" }));

    fireEvent.click(screen.getByRole("button", { name: "Back to quotas" }));

    await waitFor(() =>
      expect(screen.queryByRole("region", { name: "Claude" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("region", { name: "OpenAI" })).toBeInTheDocument();
    expect(screen.getByText(/1 provider hidden/)).toBeInTheDocument();
    expect(loadPreferences().enabledProviders).not.toContain("claude");
  });

  it("disconnects an account only from EyeUrAI and can reconnect it", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    const settings = await screen.findByRole("region", { name: "Settings" });
    fireEvent.click(
      within(settings).getByRole("button", {
        name: "Disconnect Claude account marbin@hey.com from EyeUrAI",
      }),
    );

    await waitFor(() =>
      expect(loadPreferences().disconnectedAccounts).toEqual([
        { id: "claude-personal", provider: "claude", label: "marbin@hey.com" },
      ]),
    );
    expect(
      within(settings).getByRole("button", {
        name: "Reconnect Claude account marbin@hey.com to EyeUrAI",
      }),
    ).toBeInTheDocument();

    fireEvent.click(
      within(settings).getByRole("button", {
        name: "Reconnect Claude account marbin@hey.com to EyeUrAI",
      }),
    );
    await waitFor(() => expect(loadPreferences().disconnectedAccounts).toEqual([]));
  });

  it("does not resurrect a disconnected CLI principal as a provider error row", async () => {
    const demoSnapshot = demo.createDemoSnapshot();
    const claudeTemplate = demoSnapshot.accounts.find((account) => account.provider === "claude");
    if (!claudeTemplate) throw new Error("demo snapshot needs a Claude account");

    vi.spyOn(ipc, "isTauri").mockReturnValue(true);
    vi.spyOn(ipc, "fetchSnapshot").mockResolvedValue({
      ...demoSnapshot,
      accounts: [
        {
          ...claudeTemplate,
          id: "claude-status-0",
          label: "Claude",
          status: "error",
          message: "No Claude Code login was found.",
          windows: [],
        },
        ...demoSnapshot.accounts.filter((account) => account.provider === "openai").slice(0, 1),
      ],
    });
    vi.spyOn(ipc, "subscribeToSnapshots").mockResolvedValue(() => {});
    vi.spyOn(ipc, "subscribeToRefreshRequests").mockResolvedValue(() => {});
    seedPreferences({
      disconnectedAccounts: [
        {
          id: "claude:principal-fingerprint",
          provider: "claude",
          label: "person@example.com",
        },
      ],
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "OpenAI" })).toBeInTheDocument();
    expect(screen.queryByText("No Claude Code login was found.")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Claude" })).not.toBeInTheDocument();
  });

  it("distinguishes the current terminal login from retained accounts in settings", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    const settings = await screen.findByRole("region", { name: "Settings" });

    expect(within(settings).getByText("Current terminal login · Max 20×")).toBeInTheDocument();
    expect(
      within(settings).getByText("Retained account · Last known data · Team · seat 4"),
    ).toBeInTheDocument();
  });

  it("opens provider-specific add-account guidance", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    const dialog = screen.getByRole("dialog", { name: "Add account" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Claude" }));
    expect(within(dialog).getByText(/credentials stay/i)).toBeInTheDocument();
    expect(
      within(dialog).getByText(/separate profile for each account/i),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText(/terminal login is picked up automatically/i),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Add Claude account" })).toBeInTheDocument();
  });

  it("keeps global shortcuts behind a modal and dismisses only the modal with Escape", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    expect(screen.getByRole("dialog", { name: "Add account" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "," });
    expect(screen.getByRole("region", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Add account" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Add account" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Settings" })).toBeInTheDocument();
  });

  it("adds a Claude account through an isolated EyeUrAI-owned profile", async () => {
    let completeLogin: ((event: ipc.ProfileLoginEvent) => void) | null = null;
    vi.spyOn(ipc, "subscribeToClaudeLogin").mockImplementation(async (listener) => {
      completeLogin = listener;
      return () => {};
    });
    const startLogin = vi
      .spyOn(ipc, "startClaudeAccountLogin")
      .mockResolvedValue({ profileId: "profile-claude-second" });
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    const dialog = screen.getByRole("dialog", { name: "Add account" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Claude" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Add Claude account" }));

    await waitFor(() => expect(startLogin).toHaveBeenCalledTimes(1));
    expect(
      await within(dialog).findByText(/finish signing into the Claude account/i),
    ).toBeInTheDocument();

    act(() => {
      completeLogin?.({ profileId: "some-other-profile", success: true });
    });
    expect(within(dialog).getByText(/finish signing into the Claude account/i)).toBeInTheDocument();

    act(() => {
      completeLogin?.({ profileId: "profile-claude-second", success: true });
    });
    expect(
      await within(dialog).findByText(/usage can now refresh independently/i),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Done" })).toBeInTheDocument();
  });

  it("reports a failed Claude sign-in with its message", async () => {
    let completeLogin: ((event: ipc.ProfileLoginEvent) => void) | null = null;
    vi.spyOn(ipc, "subscribeToClaudeLogin").mockImplementation(async (listener) => {
      completeLogin = listener;
      return () => {};
    });
    vi.spyOn(ipc, "startClaudeAccountLogin").mockResolvedValue({
      profileId: "profile-claude-fail",
    });
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));
    const dialog = screen.getByRole("dialog", { name: "Add account" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Claude" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Add Claude account" }));
    await within(dialog).findByText(/finish signing into the Claude account/i);

    act(() => {
      completeLogin?.({
        profileId: "profile-claude-fail",
        success: false,
        message: "Claude sign-in was cancelled.",
      });
    });
    expect(await within(dialog).findByText("Claude sign-in was cancelled.")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("adds a Codex account through an isolated provider-owned profile", async () => {
    let completeLogin: ((event: ipc.CodexLoginEvent) => void) | null = null;
    vi.spyOn(ipc, "subscribeToCodexLogin").mockImplementation(async (listener) => {
      completeLogin = listener;
      return () => {};
    });
    const startLogin = vi
      .spyOn(ipc, "startCodexAccountLogin")
      .mockResolvedValue({ profileId: "profile-second" });
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    const dialog = screen.getByRole("dialog", { name: "Add account" });
    fireEvent.click(within(dialog).getByRole("button", { name: "OpenAI" }));
    expect(within(dialog).getByText(/separate Codex profile/i)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Add Codex account" }));

    await waitFor(() => expect(startLogin).toHaveBeenCalledTimes(1));
    expect(
      await within(dialog).findByText(/finish signing into the Codex account/i),
    ).toBeInTheDocument();

    act(() => {
      completeLogin?.({ profileId: "some-other-profile", success: true });
    });
    expect(within(dialog).getByText(/finish signing into the Codex account/i)).toBeInTheDocument();

    act(() => {
      completeLogin?.({ profileId: "profile-second", success: true });
    });
    expect(
      await within(dialog).findByText(/usage can now refresh independently/i),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Done" })).toBeInTheDocument();
  });

  it("correlates an early Codex completion after the listener is ready", async () => {
    const callOrder: string[] = [];
    let completeLogin: ((event: ipc.CodexLoginEvent) => void) | null = null;
    let resolveStart!: (value: ipc.CodexLoginStarted) => void;
    const startResult = new Promise<ipc.CodexLoginStarted>((resolve) => {
      resolveStart = resolve;
    });
    vi.spyOn(ipc, "subscribeToCodexLogin").mockImplementation(async (listener) => {
      callOrder.push("listen");
      completeLogin = listener;
      return () => {};
    });
    vi.spyOn(ipc, "startCodexAccountLogin").mockImplementation(async () => {
      callOrder.push("start");
      return await startResult;
    });
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));
    const dialog = screen.getByRole("dialog", { name: "Add account" });
    fireEvent.click(within(dialog).getByRole("button", { name: "OpenAI" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Add Codex account" }));

    await waitFor(() => expect(callOrder).toEqual(["listen", "start"]));
    act(() => {
      completeLogin?.({ profileId: "unrelated-profile", success: true });
      completeLogin?.({ profileId: "profile-early", success: true });
    });
    expect(within(dialog).getByText(/starting the Codex sign-in service/i)).toBeInTheDocument();

    await act(async () => {
      resolveStart({ profileId: "profile-early" });
      await startResult;
    });
    expect(
      await within(dialog).findByText(/usage can now refresh independently/i),
    ).toBeInTheDocument();
  });

  it("asks before reading local token logs and remembers consent", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    const usage = screen.getByRole("region", { name: "Local token usage" });
    fireEvent.click(within(usage).getByRole("button", { name: "Review access" }));
    const dialog = screen.getByRole("dialog", { name: "Local usage access" });
    expect(within(dialog).getByText("~/.claude/projects")).toBeInTheDocument();
    expect(within(dialog).getByText("~/.codex/sessions")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Allow local scan" }));
    await waitFor(() => expect(loadPreferences().localUsageEnabled).toBe(true));
    expect((await within(usage).findAllByText("669M")).length).toBeGreaterThan(0);
    expect(within(usage).getByRole("img", { name: "Daily processed tokens over 7 days" })).toBeInTheDocument();
    expect(within(usage).getByText("gpt-5.6-sol")).toBeInTheDocument();

    fireEvent.click(within(usage).getByRole("button", { name: "30 days" }));
    expect((await within(usage).findAllByText("2.9B")).length).toBeGreaterThan(0);

    fireEvent.click(within(usage).getByRole("button", { name: "day" }));
    expect(within(usage).getByRole("table", { name: "day usage breakdown" })).toBeInTheDocument();
  });

  it("ignores an older local-usage range scan that finishes last", async () => {
    let resolveSeven!: (value: ReturnType<typeof demo.createDemoLocalUsage>) => void;
    let resolveNinety!: (value: ReturnType<typeof demo.createDemoLocalUsage>) => void;
    const seven = new Promise<ReturnType<typeof demo.createDemoLocalUsage>>((resolve) => {
      resolveSeven = resolve;
    });
    const ninety = new Promise<ReturnType<typeof demo.createDemoLocalUsage>>((resolve) => {
      resolveNinety = resolve;
    });

    vi.spyOn(ipc, "isTauri").mockReturnValue(true);
    vi.spyOn(ipc, "fetchSnapshot").mockResolvedValue(demo.createDemoSnapshot());
    vi.spyOn(ipc, "subscribeToSnapshots").mockResolvedValue(() => {});
    vi.spyOn(ipc, "subscribeToRefreshRequests").mockResolvedValue(() => {});
    const fetchLocalUsage = vi.spyOn(ipc, "fetchLocalUsage").mockImplementation(async (days) => {
      return await (days === 90 ? ninety : seven);
    });

    seedPreferences({ localUsageEnabled: true });
    render(<App />);
    const usage = await screen.findByRole("region", { name: "Local token usage" });
    await waitFor(() => expect(fetchLocalUsage).toHaveBeenCalledWith(7));
    fireEvent.click(within(usage).getByRole("button", { name: "90 days" }));
    await waitFor(() => expect(fetchLocalUsage).toHaveBeenCalledWith(90));

    await act(async () => {
      resolveNinety(demo.createDemoLocalUsage(Date.now(), 90));
      await ninety;
    });
    expect(
      await within(usage).findByRole("img", { name: "Daily processed tokens over 90 days" }),
    ).toBeInTheDocument();

    await act(async () => {
      resolveSeven(demo.createDemoLocalUsage(Date.now(), 7));
      await seven;
    });
    expect(
      within(usage).getByRole("img", { name: "Daily processed tokens over 90 days" }),
    ).toBeInTheDocument();
    expect(within(usage).getByRole("button", { name: "90 days" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("shows a helpful empty state when nothing is visible", async () => {
    seedPreferences({ enabledProviders: [] });
    render(<App />);

    expect(await screen.findByText("Every provider is hidden")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Manage providers" }));
    expect(await screen.findByRole("region", { name: "Settings" })).toBeInTheDocument();
  });

  it("shows the packaged app version in settings", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    expect(
      await screen.findByText(
        (_, element) =>
          element?.tagName === "P" &&
          element.textContent?.startsWith(`EyeUrAI v${__APP_VERSION__} ·`) === true,
      ),
    ).toBeInTheDocument();
  });

  it("summarises the worst quota in the status bar", async () => {
    seedPreferences();
    render(<App />);
    // The stale Claude team account sits at 92.9%, but only live rows may
    // drive the headline. Gemini's live daily window is the current peak.
    expect(await screen.findByText(/Gemini · 2.5 Pro · daily · 91%/)).toBeInTheDocument();
    expect(screen.queryByText(/Claude · Session · 93%/)).not.toBeInTheDocument();
  });

  it("pins the exact Home quota, replaces it, and unpins it on a second click", async () => {
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Session (5h) quota for Claude · marbin@hey.com to menu bar",
      }),
    );
    await waitFor(() =>
      expect(loadPreferences().pinnedQuota).toEqual({
        accountId: "claude-personal",
        windowId: "session",
      }),
    );
    expect(
      screen.getByRole("button", {
        name: "Unpin Session (5h) quota for Claude · marbin@hey.com from menu bar",
      }),
    ).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Session (5h) quota for Claude · eng@nimbus.dev to menu bar",
      }),
    );
    await waitFor(() =>
      expect(loadPreferences().pinnedQuota).toEqual({
        accountId: "claude-team",
        windowId: "session",
      }),
    );
    expect(
      screen.getByRole("button", {
        name: "Pin Session (5h) quota for Claude · marbin@hey.com to menu bar",
      }),
    ).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Weekly · all models quota for Claude · marbin@hey.com to menu bar",
      }),
    );
    await waitFor(() =>
      expect(loadPreferences().pinnedQuota).toEqual({
        accountId: "claude-personal",
        windowId: "weekly-all",
      }),
    );
    expect(
      screen.getByRole("button", {
        name: "Pin Session (5h) quota for Claude · marbin@hey.com to menu bar",
      }),
    ).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(
      screen.getByRole("button", {
        name: "Unpin Weekly · all models quota for Claude · marbin@hey.com from menu bar",
      }),
    );
    await waitFor(() => expect(loadPreferences().pinnedQuota).toBeNull());
  });

  it("updates pinned menu-bar usage on refresh and restores the logo on unpin", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    seedPreferences({
      pinnedQuota: { accountId: "claude-personal", windowId: "session" },
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith("5h", 62));

    fireEvent.click(screen.getByRole("button", { name: "Refresh quotas" }));
    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith("5h", 63));

    fireEvent.click(
      screen.getByRole("button", {
        name: "Unpin Session (5h) quota for Claude · marbin@hey.com from menu bar",
      }),
    );
    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith(null, null));
  });

  it("switches an existing Codex weekly pin to Claude five-hour usage", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    seedPreferences({
      pinnedQuota: { accountId: "openai-pro", windowId: "codex-weekly" },
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Session (5h) quota for Claude · marbin@hey.com to menu bar",
      }),
    );

    await waitFor(() =>
      expect(loadPreferences().pinnedQuota).toEqual({
        accountId: "claude-personal",
        windowId: "session",
      }),
    );
    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith("5h", 62));
  });

  it("pins a live reset countdown to the menu bar", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    seedPreferences();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin reset timer for Session (5h) quota for Claude · marbin@hey.com to menu bar",
      }),
    );

    await waitFor(() =>
      expect(loadPreferences().pinnedQuota).toEqual({
        accountId: "claude-personal",
        windowId: "session",
        display: "reset",
      }),
    );
    await waitFor(() =>
      expect(traySpy).toHaveBeenLastCalledWith(
        "5h",
        null,
        expect.stringMatching(/^\d+(?:h(?: \d+m)?|m|s)$/),
      ),
    );
  });

  it("keeps a reset pin visible when the provider temporarily omits that window", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    vi.spyOn(demo, "refreshDemoSnapshot").mockImplementation((snapshot, now) => ({
      ...snapshot,
      generatedAt: new Date(now ?? Date.now()).toISOString(),
      accounts: snapshot.accounts.map((account) =>
        account.id === "claude-personal"
          ? { ...account, windows: account.windows.filter((window) => window.id !== "session") }
          : account,
      ),
    }));
    seedPreferences({
      pinnedQuota: { accountId: "claude-personal", windowId: "session", display: "reset" },
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    await waitFor(() =>
      expect(traySpy).toHaveBeenLastCalledWith(
        "5h",
        null,
        expect.stringMatching(/^\d+(?:h(?: \d+m)?|m|s)$/),
      ),
    );

    fireEvent.click(screen.getByRole("button", { name: "Refresh quotas" }));
    await waitFor(() =>
      expect(
        screen.queryByRole("button", {
          name: "Unpin reset timer for Session (5h) quota for Claude · marbin@hey.com from menu bar",
        }),
      ).not.toBeInTheDocument(),
    );

    expect(traySpy).toHaveBeenLastCalledWith(
      "5h",
      null,
      expect.stringMatching(/^\d+(?:h(?: \d+m)?|m|s)$/),
    );
  });

  it("keeps a temporarily unavailable pin while showing the logo", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    seedPreferences({ pinnedQuota: { accountId: "missing", windowId: "weekly" } });
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith(null, null));
    expect(loadPreferences().pinnedQuota).toEqual({
      accountId: "missing",
      windowId: "weekly",
    });

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    expect(
      await screen.findByRole("button", {
        name: "Unavailable. Unpin quota and restore EyeUrAI logo",
      }),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Unavailable. Unpin quota and restore EyeUrAI logo",
      }),
    );
    await waitFor(() => expect(loadPreferences().pinnedQuota).toBeNull());
  });

  it("keeps a stale pin labelled without presenting its percentage as live", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    seedPreferences({
      pinnedQuota: { accountId: "claude-team", windowId: "session" },
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith("5h", null));
    expect(loadPreferences().pinnedQuota).toEqual({
      accountId: "claude-team",
      windowId: "session",
    });
  });

});
