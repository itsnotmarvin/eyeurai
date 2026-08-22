// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { Account } from "../../types/quota";
import { StatusBar } from "../StatusBar";

function account(status: Account["status"], percentUsed: number): Account {
  return {
    id: `account-${status}`,
    provider: "claude",
    label: status,
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

afterEach(cleanup);

describe("StatusBar", () => {
  it("uses only fresh quota windows for the live headline", () => {
    render(
      <StatusBar
        accounts={[account("stale", 99), account("fresh", 42)]}
        generatedAt="2026-08-20T12:00:00Z"
        now={Date.parse("2026-08-20T12:01:00Z")}
        warnThreshold={75}
        criticalThreshold={90}
        mode="live"
      />,
    );

    expect(screen.getByText("Claude · Weekly · 42%")).toBeInTheDocument();
    expect(screen.queryByText(/99%/)).not.toBeInTheDocument();
  });

  it("does not present unconfigured or unsupported providers as emergencies", () => {
    render(
      <StatusBar
        accounts={[account("unconfigured", 0), account("unsupported", 0)]}
        generatedAt={null}
        now={Date.parse("2026-08-20T12:01:00Z")}
        warnThreshold={75}
        criticalThreshold={90}
        mode="live"
      />,
    );

    expect(screen.getByText("No quota data yet")).toBeInTheDocument();
    expect(screen.queryByText(/need attention/)).not.toBeInTheDocument();
  });
});
