// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as autostart from "@tauri-apps/plugin-autostart";
import { useLaunchAtLogin } from "./useLaunchAtLogin";

vi.mock("../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/plugin-autostart", () => ({
  isEnabled: vi.fn(),
  enable: vi.fn(),
  disable: vi.fn(),
}));

describe("useLaunchAtLogin", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("enables the login item once when its default has not been applied", async () => {
    vi.mocked(autostart.isEnabled).mockResolvedValue(true);
    vi.mocked(autostart.isEnabled).mockResolvedValueOnce(false);
    const onDefaultApplied = vi.fn();

    const { result, rerender } = renderHook(
      ({ enableByDefault }) => useLaunchAtLogin({ enableByDefault, onDefaultApplied }),
      { initialProps: { enableByDefault: true } },
    );

    await waitFor(() => expect(result.current.busy).toBe(false));
    expect(autostart.enable).toHaveBeenCalledTimes(1);
    expect(result.current.enabled).toBe(true);
    expect(onDefaultApplied).toHaveBeenCalledTimes(1);

    rerender({ enableByDefault: false });
    await waitFor(() => expect(autostart.isEnabled).toHaveBeenCalledTimes(3));
    expect(autostart.enable).toHaveBeenCalledTimes(1);
  });

  it("preserves opt-out after the one-time default was recorded", async () => {
    vi.mocked(autostart.isEnabled).mockResolvedValue(false);

    const { result } = renderHook(() => useLaunchAtLogin({ enableByDefault: false }));

    await waitFor(() => expect(result.current.busy).toBe(false));
    expect(result.current.enabled).toBe(false);
    expect(autostart.enable).not.toHaveBeenCalled();
  });

  it("still lets the user disable startup from Settings", async () => {
    vi.mocked(autostart.isEnabled).mockResolvedValueOnce(true).mockResolvedValueOnce(false);

    const { result } = renderHook(() => useLaunchAtLogin({ enableByDefault: false }));
    await waitFor(() => expect(result.current.busy).toBe(false));

    await act(async () => result.current.setEnabled(false));

    expect(autostart.disable).toHaveBeenCalledTimes(1);
    expect(result.current.enabled).toBe(false);
  });
});
