// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { QuotaWindow } from "../../types/quota";
import { QuotaBar } from "../QuotaBar";

const NOW = Date.parse("2025-03-04T12:00:00.000Z");

function makeWindow(overrides: Partial<QuotaWindow> = {}): QuotaWindow {
  return {
    id: "session",
    label: "Session",
    kind: "session",
    unit: "tokens",
    used: 118_400,
    limit: 190_000,
    percentUsed: 62.3,
    resetsAt: new Date(NOW + 2 * 3_600_000 + 14 * 60_000).toISOString(),
    note: "5h",
    ...overrides,
  };
}

function renderBar(
  window: QuotaWindow,
  isPinned = false,
  onTogglePin?: (display: "usage" | "reset") => void,
  pinnedDisplay: "usage" | "reset" = "usage",
) {
  return render(
    <ul>
      <QuotaBar
        window={window}
        now={NOW}
        warnThreshold={75}
        criticalThreshold={90}
        accountName="Claude · work@acme.com"
        isPinned={isPinned}
        pinnedDisplay={pinnedDisplay}
        onTogglePin={onTogglePin}
      />
    </ul>,
  );
}

afterEach(cleanup);

describe("QuotaBar", () => {
  it("exposes a semantic progress bar with account context", () => {
    renderBar(makeWindow());

    const bar = screen.getByRole("progressbar", {
      name: "Session usage for Claude · work@acme.com",
    });
    expect(bar).toHaveAttribute("aria-valuenow", "62");
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
    expect(bar).toHaveAttribute(
      "aria-valuetext",
      "62 percent used, 118.4K / 190K tokens, resets in 2h 14m",
    );
  });

  it("renders percentage, usage and countdown text", () => {
    renderBar(makeWindow());

    expect(screen.getByText("62%")).toBeInTheDocument();
    expect(screen.getByText("118.4K / 190K tokens")).toBeInTheDocument();
    expect(screen.getByText("resets in 2h 14m")).toBeInTheDocument();
    expect(screen.getByText("5h")).toBeInTheDocument();
  });

  it("marks severity from the user thresholds", () => {
    const { container, rerender } = renderBar(makeWindow({ percentUsed: 80 }));
    expect(container.querySelector(".quota")).toHaveAttribute("data-severity", "warn");

    rerender(
      <ul>
        <QuotaBar
          window={makeWindow({ percentUsed: 94 })}
          now={NOW}
          warnThreshold={75}
          criticalThreshold={90}
          accountName="Claude · work@acme.com"
        />
      </ul>,
    );
    expect(container.querySelector(".quota")).toHaveAttribute("data-severity", "critical");
  });

  it("handles windows that never reset", () => {
    renderBar(makeWindow({ resetsAt: null }));
    expect(screen.getByText("no reset")).toBeInTheDocument();
  });

  it("sets the fill width from the clamped percentage", () => {
    const { container } = renderBar(makeWindow({ percentUsed: 143 }));
    expect(container.querySelector<HTMLElement>(".quota__fill")?.style.width).toBe("100%");
  });

  it("exposes the whole quota as an accessible pin toggle", () => {
    const onTogglePin = vi.fn();
    const { container } = renderBar(makeWindow(), true, onTogglePin);

    const toggle = screen.getByRole("button", {
      name: "Unpin Session (5h) quota for Claude · work@acme.com from menu bar",
    });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(container.querySelector(".quota")).toHaveAttribute("data-pinned", "true");
    expect(screen.getByText("Pinned")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(onTogglePin).toHaveBeenCalledWith("usage");
  });

  it("pins the reset countdown independently from usage", () => {
    const onTogglePin = vi.fn();
    const { container } = renderBar(makeWindow(), true, onTogglePin, "reset");

    const timerToggle = screen.getByRole("button", {
      name: "Unpin reset timer for Session (5h) quota for Claude · work@acme.com from menu bar",
    });
    expect(timerToggle).toHaveAttribute("aria-pressed", "true");
    expect(container.querySelector(".quota")).toHaveAttribute("data-pinned", "true");
    expect(screen.getByText("Timer pinned")).toBeInTheDocument();

    fireEvent.click(timerToggle);
    expect(onTogglePin).toHaveBeenCalledWith("reset");
  });
});
