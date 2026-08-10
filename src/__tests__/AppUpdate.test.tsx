// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "../App";
import * as appUpdates from "../lib/appUpdates";
import { DEFAULT_PREFERENCES, clearPreferences, savePreferences } from "../lib/preferences";

beforeEach(() => {
  clearPreferences();
  savePreferences({ ...DEFAULT_PREFERENCES, onboardingCompleted: true });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("application updates", () => {
  it("shows an available update and installs it after confirmation", async () => {
    vi.spyOn(appUpdates, "checkForAppUpdate").mockResolvedValue({
      currentVersion: "1.0.0",
      version: "1.0.1",
      notes: "Sharper quota refreshes and a smaller menu-bar footprint.",
    });
    const installSpy = vi
      .spyOn(appUpdates, "installAppUpdate")
      .mockImplementation(async (onProgress) => {
        onProgress({ stage: "downloading", percent: 48 });
        onProgress({ stage: "installing", percent: 100 });
      });

    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });
    const updateButton = await screen.findByRole("button", {
      name: "Update available: EyeUrAI 1.0.1",
    });
    fireEvent.click(updateButton);

    const dialog = screen.getByRole("dialog", { name: "EyeUrAI update" });
    expect(within(dialog).getByText("EyeUrAI 1.0.1 is available")).toBeInTheDocument();
    expect(within(dialog).getByText(/Sharper quota refreshes/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Update & restart" }));

    await waitFor(() => expect(installSpy).toHaveBeenCalledOnce());
    expect(within(dialog).getByRole("progressbar", { name: "Installing update" }))
      .toHaveAttribute("aria-valuenow", "100");
  });
});
