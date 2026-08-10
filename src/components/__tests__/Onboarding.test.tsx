// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_PREFERENCES } from "../../lib/preferences";
import { Onboarding } from "../Onboarding";

afterEach(cleanup);

function setup(options: { granted?: boolean } = {}) {
  const onComplete = vi.fn();
  const requestPermission = vi.fn().mockResolvedValue(options.granted ?? true);
  render(
    <Onboarding
      initial={DEFAULT_PREFERENCES}
      onComplete={onComplete}
      requestPermission={requestPermission}
    />,
  );
  return { onComplete, requestPermission };
}

describe("Onboarding", () => {
  it("lists every provider as an accessible checkbox, all pre-selected", () => {
    setup();
    const checkboxes = screen.getAllByRole("checkbox");
    expect(checkboxes).toHaveLength(4);
    expect(checkboxes.map((node) => node.textContent)).toEqual([
      expect.stringContaining("Claude"),
      expect.stringContaining("OpenAI"),
      expect.stringContaining("OpenRouter"),
      expect.stringContaining("Gemini"),
    ]);
    for (const checkbox of checkboxes) {
      expect(checkbox).toHaveAttribute("aria-checked", "true");
    }
  });

  it("blocks continuing until at least one provider is picked", () => {
    setup();
    for (const checkbox of screen.getAllByRole("checkbox")) {
      fireEvent.click(checkbox);
    }

    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    expect(screen.getByText("Pick at least one provider to continue.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: /Gemini/ }));
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
  });

  it("keeps providers in canonical order regardless of click order", () => {
    const { onComplete } = setup();
    fireEvent.click(screen.getByRole("checkbox", { name: /Claude/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /OpenRouter/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /OpenRouter/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Claude/ }));

    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(onComplete).toHaveBeenCalledWith(
      expect.objectContaining({
        enabledProviders: ["claude", "openai", "openrouter", "gemini"],
        onboardingCompleted: true,
      }),
    );
  });

  it("requests permission before enabling notifications and saves thresholds", async () => {
    const { onComplete, requestPermission } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));

    const toggle = screen.getByRole("switch", { name: "Desktop notifications" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"));
    expect(requestPermission).toHaveBeenCalledTimes(1);

    const warn = screen.getByLabelText("Warn at");
    fireEvent.change(warn, { target: { value: "88" } });

    fireEvent.click(screen.getByRole("button", { name: "Start monitoring" }));
    expect(onComplete).toHaveBeenCalledWith(
      expect.objectContaining({
        notificationsEnabled: true,
        warnThreshold: 88,
        criticalThreshold: 90,
        onboardingCompleted: true,
      }),
    );
  });

  it("explains when the OS denies notification permission", async () => {
    const { requestPermission } = setup({ granted: false });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    fireEvent.click(screen.getByRole("switch", { name: "Desktop notifications" }));

    await waitFor(() => expect(requestPermission).toHaveBeenCalled());
    expect(await screen.findByText(/system denied notifications/i)).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Desktop notifications" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });

  it("moves back to the provider step", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getAllByRole("checkbox")).toHaveLength(4);
  });
});
