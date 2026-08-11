// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DEFAULT_PREFERENCES } from "../../lib/preferences";
import { SettingsView } from "../SettingsView";

describe("SettingsView", () => {
  it("saves a new refresh interval and refreshes immediately", () => {
    const onChange = vi.fn();
    const onRefreshAccounts = vi.fn();

    render(
      <SettingsView
        preferences={DEFAULT_PREFERENCES}
        onChange={onChange}
        mode="demo"
        onRerunSetup={vi.fn()}
        accounts={[]}
        onDisconnectAccount={vi.fn()}
        onReconnectAccount={vi.fn()}
        onRefreshAccounts={onRefreshAccounts}
        onRequestLocalUsage={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByRole("combobox", { name: "Auto-refresh interval" }), {
      target: { value: "15" },
    });

    expect(onChange).toHaveBeenCalledWith({
      ...DEFAULT_PREFERENCES,
      refreshIntervalSeconds: 15,
    });
    expect(onRefreshAccounts).toHaveBeenCalledTimes(1);
  });
});
