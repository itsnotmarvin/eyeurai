// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "../App";
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

  it("shows a helpful empty state when nothing is visible", async () => {
    seedPreferences({ enabledProviders: [] });
    render(<App />);

    expect(await screen.findByText("Every provider is hidden")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Manage providers" }));
    expect(await screen.findByRole("region", { name: "Settings" })).toBeInTheDocument();
  });

  it("summarises the worst quota in the status bar", async () => {
    seedPreferences();
    render(<App />);
    // Claude · Session sits at 92.9% in the demo data for the stale team account.
    expect(await screen.findByText(/Claude · Session · 93%/)).toBeInTheDocument();
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

  it("does not present a retained stale quota as live in the menu bar", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    seedPreferences({
      pinnedQuota: { accountId: "claude-team", windowId: "session" },
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith(null, null));
    expect(loadPreferences().pinnedQuota).toEqual({
      accountId: "claude-team",
      windowId: "session",
    });
  });

});
