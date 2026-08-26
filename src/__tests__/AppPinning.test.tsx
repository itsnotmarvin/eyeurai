// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/format", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/format")>();
  return { ...actual, displayPercent: () => 50 };
});

import { App } from "../App";
import * as ipc from "../lib/ipc";
import {
  DEFAULT_PREFERENCES,
  clearPreferences,
  savePreferences,
} from "../lib/preferences";

beforeEach(() => {
  clearPreferences();
  savePreferences({ ...DEFAULT_PREFERENCES, onboardingCompleted: true });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("App quota pinning", () => {
  it("resends an identical tray value when switching from OpenAI 5-hour to Claude 5-hour", async () => {
    const traySpy = vi.spyOn(ipc, "setTrayDisplay").mockResolvedValue();
    render(<App />);
    await screen.findByRole("heading", { name: "Claude" });

    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Codex · local (5h) quota for OpenAI · demo.openai@example.com to menu bar",
      }),
    );
    await waitFor(() => expect(traySpy).toHaveBeenLastCalledWith("5h", 50));

    traySpy.mockClear();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Pin Session (5h) quota for Claude · demo.personal@example.com to menu bar",
      }),
    );

    await waitFor(() => expect(traySpy).toHaveBeenCalledWith("5h", 50));
  });
});
