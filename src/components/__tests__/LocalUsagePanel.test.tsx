// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { LocalUsageSnapshot } from "../../types/quota";
import { LocalUsagePanel } from "../LocalUsagePanel";

const EMPTY_USAGE: LocalUsageSnapshot = {
  generatedAt: "2026-08-20T12:00:00Z",
  rangeDays: 7,
  truncated: false,
  processedTokens: 0,
  uncachedInputTokens: 0,
  cachedInputTokens: 0,
  cacheWriteInputTokens: 0,
  outputTokens: 0,
  reasoningOutputTokens: 0,
  observations: 0,
  sessions: 0,
  providers: [],
  daily: [],
  models: [],
};

afterEach(cleanup);

describe("LocalUsagePanel", () => {
  it("keeps wider ranges available when the current range is empty", () => {
    const onRangeChange = vi.fn();
    render(
      <LocalUsagePanel
        enabled
        usage={EMPTY_USAGE}
        loading={false}
        error={null}
        rangeDays={7}
        onRangeChange={onRangeChange}
        onReviewAccess={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "30 days" }));
    expect(onRangeChange).toHaveBeenCalledWith(30);
  });

  it("warns when safety limits make the totals incomplete", () => {
    render(
      <LocalUsagePanel
        enabled
        usage={{ ...EMPTY_USAGE, truncated: true }}
        loading={false}
        error={null}
        rangeDays={7}
        onRangeChange={vi.fn()}
        onReviewAccess={vi.fn()}
      />,
    );

    expect(screen.getByText(/total is partial/i)).toBeInTheDocument();
  });
});
