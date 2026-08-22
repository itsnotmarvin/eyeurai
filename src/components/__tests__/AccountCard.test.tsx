// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Account } from "../../types/quota";
import { AccountCard } from "../AccountCard";

const NOW = Date.parse("2025-03-04T12:00:00.000Z");

function makeAccount(overrides: Partial<Account> = {}): Account {
  return {
    id: "claude-1",
    provider: "claude",
    label: "work@acme.com",
    plan: "Max 20×",
    source: "oauth",
    status: "fresh",
    message: null,
    updatedAt: new Date(NOW - 3 * 60_000).toISOString(),
    windows: [
      {
        id: "session",
        label: "Session",
        kind: "session",
        unit: "percent",
        used: 62,
        limit: 100,
        percentUsed: 62,
        resetsAt: new Date(NOW + 3_600_000).toISOString(),
        note: null,
      },
      {
        id: "weekly",
        label: "Weekly",
        kind: "weekly",
        unit: "percent",
        used: 93,
        limit: 100,
        percentUsed: 93,
        resetsAt: new Date(NOW + 2 * 86_400_000).toISOString(),
        note: null,
      },
    ],
    ...overrides,
  };
}

function renderCard(account: Account, onRetry?: () => void) {
  return render(
    <AccountCard
      account={account}
      now={NOW}
      warnThreshold={75}
      criticalThreshold={90}
      onRetry={onRetry}
    />,
  );
}

afterEach(cleanup);

describe("AccountCard", () => {
  it("shows identity, plan and stacked quota bars", () => {
    renderCard(makeAccount());

    expect(screen.getByRole("article", { name: "Claude · work@acme.com" })).toBeInTheDocument();
    expect(screen.getByText("work@acme.com")).toBeInTheDocument();
    expect(screen.getByText("Max 20×")).toBeInTheDocument();
    expect(screen.getByText("OAuth")).toBeInTheDocument();
    expect(screen.getAllByRole("progressbar")).toHaveLength(2);
    expect(screen.getByText("updated 3m ago")).toBeInTheDocument();
  });

  it("reports a fresh state", () => {
    renderCard(makeAccount());
    expect(screen.getByText("Live")).toBeInTheDocument();
  });

  it("presents unconfigured providers neutrally", () => {
    renderCard(makeAccount({ status: "unconfigured", windows: [] }));
    expect(screen.getByText("Not connected")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("surfaces stale data with its explanation", () => {
    renderCard(makeAccount({ status: "stale", message: "Claude Code was not running." }));

    expect(screen.getByText("Stale")).toBeInTheDocument();
    expect(screen.getByText("Claude Code was not running.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /retry/i })).not.toBeInTheDocument();
  });

  it("shows retained CLI observations as last known instead of live", () => {
    renderCard(
      makeAccount({
        source: "cli",
        isCliActive: false,
        status: "stale",
        message: "Saved from the previous terminal login.",
      }),
    );

    expect(screen.getByText("Last known")).toBeInTheDocument();
    expect(screen.queryByText("Live")).not.toBeInTheDocument();
    expect(screen.getAllByRole("progressbar")).toHaveLength(2);
    expect(screen.getByText("Saved from the previous terminal login.")).toBeInTheDocument();
  });

  it("keeps an independently connected CLI profile live even when it is not primary", () => {
    renderCard(makeAccount({ source: "cli", isCliActive: false }));

    expect(screen.getByText("Live")).toBeInTheDocument();
    expect(screen.queryByText("Cached")).not.toBeInTheDocument();
  });

  it("offers a retry action on error accounts", () => {
    const onRetry = vi.fn();
    renderCard(
      makeAccount({ status: "error", message: "Credentials expired.", windows: [] }),
      onRetry,
    );

    expect(screen.getByRole("alert")).toHaveTextContent("Credentials expired.");
    expect(screen.getByText("No quota windows reported yet.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("offers a diagnosed reconnect action instead of a generic retry", () => {
    const onRetry = vi.fn();
    const onRemediate = vi.fn();
    const account = makeAccount({
      source: "cli",
      isCliActive: false,
      status: "error",
      message: "This is not the current Claude Code account.",
      remediation: {
        id: "opaque-plan",
        title: "Reconnect this Claude account?",
        detail: "Choose how to reconnect.",
        choices: [
          {
            id: "managed-login",
            kind: "managed-login",
            label: "Reconnect inside EyeUrAI",
            detail: null,
            commandPreview: null,
            impact: "app-only",
          },
        ],
      },
    });
    render(
      <AccountCard
        account={account}
        now={NOW}
        warnThreshold={75}
        criticalThreshold={90}
        onRetry={onRetry}
        onRemediate={onRemediate}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Reconnect" }));
    expect(onRemediate).toHaveBeenCalledWith(account);
    expect(onRetry).not.toHaveBeenCalled();
  });

  it("pins the exact account and window selected", () => {
    const onToggleQuotaPin = vi.fn();
    render(
      <AccountCard
        account={makeAccount()}
        now={NOW}
        warnThreshold={75}
        criticalThreshold={90}
        pinnedQuota={{ accountId: "claude-1", windowId: "weekly" }}
        onToggleQuotaPin={onToggleQuotaPin}
      />,
    );

    expect(
      screen.getByRole("button", {
        name: "Unpin Weekly quota for Claude · work@acme.com from menu bar",
      }),
    ).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Session quota for Claude · work@acme.com to menu bar",
      }),
    );
    expect(onToggleQuotaPin).toHaveBeenCalledWith("claude-1", "session", "usage");
  });
});
