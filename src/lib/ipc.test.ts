import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelUpdateRelaunch,
  prepareUpdateRelaunch,
  relaunchAfterUpdate,
  resetIpcCacheForTests,
} from "./ipc";

type TauriWindow = Window & { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as TauriWindow).__TAURI_INTERNALS__;
  resetIpcCacheForTests();
  vi.doUnmock("@tauri-apps/api/core");
});

describe("native update lifecycle IPC", () => {
  it("fails closed when a Tauri-looking page cannot load the native invoke bridge", async () => {
    (window as TauriWindow).__TAURI_INTERNALS__ = {};
    vi.doMock("@tauri-apps/api/core", () => {
      throw new Error("native bridge unavailable");
    });
    resetIpcCacheForTests();

    await expect(prepareUpdateRelaunch("1.3.2")).rejects.toThrow(
      "native update preparation command",
    );
    await expect(cancelUpdateRelaunch()).rejects.toThrow("native update cancellation command");
    await expect(relaunchAfterUpdate()).rejects.toThrow("native update relaunch command");
  });
});
