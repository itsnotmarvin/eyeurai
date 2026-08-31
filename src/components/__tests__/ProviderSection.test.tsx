// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { Account } from "../../types/quota";
import { ProviderSection } from "../ProviderSection";

const NOW = Date.parse("2026-08-20T12:01:00Z");

function account(status: Account["status"], percentUsed: number, index: number): Account {
  return {
    id: `claude-${index}`,
    provider: "claude",
    label: `${status}-${index}`,
    plan: null,
    source: "oauth",
    status,
    message: null,
    updatedAt: "2026-08-20T12:00:00Z",
    windows: [
      {
        id: "weekly",
        label: "Weekly",
        kind: "weekly",
        unit: "percent",
        used: percentUsed,
        limit: 100,
        percentUsed,
        resetsAt: null,
        note: null,
      },
    ],
  };
}

function renderSection(accounts: Account[]) {
  return render(
    <ProviderSection
      provider="claude"
      accounts={accounts}
      now={NOW}
      warnThreshold={75}
      criticalThreshold={90}
    />,
  );
}

afterEach(cleanup);

describe("ProviderSection", () => {
  it("calculates the live peak from fresh accounts only", () => {
    const { container } = renderSection([
      account("fresh", 42, 1),
      account("stale", 99, 2),
      account("error", 98, 3),
      account("unsupported", 97, 4),
    ]);

    const peak = container.querySelector(".provider__peak");
    expect(peak).toHaveTextContent("42% peak");
    expect(peak).toHaveAttribute("data-severity", "normal");
  });

  it("does not show a live peak when every account is non-fresh", () => {
    const { container } = renderSection([
      account("stale", 99, 1),
      account("error", 98, 2),
      account("unsupported", 97, 3),
    ]);

    expect(container.querySelector(".provider__peak")).not.toBeInTheDocument();
  });
});
