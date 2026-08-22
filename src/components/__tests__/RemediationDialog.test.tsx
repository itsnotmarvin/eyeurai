// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Account } from "../../types/quota";
import { RemediationDialog } from "../RemediationDialog";

afterEach(cleanup);

describe("RemediationDialog", () => {
  it("separates an app-only reconnect from the global CLI switch", () => {
    const account: Account = {
      id: "claude:principal",
      provider: "claude",
      label: "work@example.com",
      plan: "Max 5x",
      source: "cli",
      isCliActive: false,
      status: "stale",
      message: "Not the current Claude Code account.",
      updatedAt: null,
      windows: [],
      remediation: {
        id: "opaque-plan",
        title: "Reconnect this Claude account?",
        detail: "Choose how to reconnect.",
        choices: [
          {
            id: "managed-login",
            kind: "managed-login",
            label: "Reconnect inside EyeUrAI",
            detail: "Does not change your terminal account.",
            commandPreview: null,
            impact: "app-only",
          },
          {
            id: "open-terminal",
            kind: "open-terminal",
            label: "Switch Claude Code account…",
            detail: "Changes the account used by Claude Code on this computer.",
            commandPreview: "claude /login",
            impact: "global-cli-identity",
          },
        ],
      },
    };

    render(
      <RemediationDialog
        account={account}
        accounts={[account]}
        plan={account.remediation!}
        onClose={vi.fn()}
        onRefresh={vi.fn()}
        onOpenSettings={vi.fn()}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Reconnect this Claude account?" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reconnect inside EyeUrAI" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Switch Claude Code account…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy claude /login" })).toBeInTheDocument();
    expect(screen.getByText(/Changes the account used by Claude Code/)).toBeInTheDocument();
  });
});
