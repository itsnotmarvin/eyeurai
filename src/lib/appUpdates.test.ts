import { afterEach, describe, expect, it, vi } from "vitest";

import {
  installAppUpdate,
  isNewerAppVersion,
  resetAppUpdateForTests,
  setPendingAppUpdateForTests,
} from "./appUpdates";
import * as ipc from "./ipc";

afterEach(async () => {
  await resetAppUpdateForTests();
  vi.restoreAllMocks();
});

describe("isNewerAppVersion", () => {
  it("accepts only a genuinely newer semantic version", () => {
    expect(isNewerAppVersion("1.2.2", "1.2.1")).toBe(true);
    expect(isNewerAppVersion("2.0.0", "1.99.99")).toBe(true);
    expect(isNewerAppVersion("1.2.1", "1.2.1")).toBe(false);
    expect(isNewerAppVersion("1.2.0", "1.2.1")).toBe(false);
  });

  it("handles prereleases and rejects malformed feed versions", () => {
    expect(isNewerAppVersion("1.2.2", "1.2.2-beta.2")).toBe(true);
    expect(isNewerAppVersion("1.2.2-beta.2", "1.2.2-beta.1")).toBe(true);
    expect(isNewerAppVersion("1.2.2-beta.1", "1.2.2-beta")).toBe(true);
    expect(isNewerAppVersion("1.2.2-beta.1", "1.2.2")).toBe(false);
    expect(isNewerAppVersion("latest", "1.2.1")).toBe(false);
  });
});

describe("installAppUpdate", () => {
  it("hands the installed update to the native relaunch path", async () => {
    const sequence: string[] = [];
    const close = vi.fn(async () => {
      sequence.push("close-resource");
    });
    setPendingAppUpdateForTests({
      currentVersion: "1.3.1",
      version: "1.3.2",
      close,
      downloadAndInstall: vi.fn(async (onEvent) => {
        sequence.push("install");
        onEvent?.({ event: "Finished" });
      }),
    });
    vi.spyOn(ipc, "prepareUpdateRelaunch").mockImplementation(async () => {
      sequence.push("prepare-relaunch");
    });
    vi.spyOn(ipc, "relaunchAfterUpdate").mockImplementation(async () => {
      sequence.push("native-relaunch");
    });
    const onProgress = vi.fn();

    await installAppUpdate(onProgress);

    expect(sequence).toEqual([
      "prepare-relaunch",
      "install",
      "close-resource",
      "native-relaunch",
    ]);
    expect(ipc.prepareUpdateRelaunch).toHaveBeenCalledWith("1.3.2");
    expect(close).toHaveBeenCalledOnce();
    expect(onProgress).toHaveBeenLastCalledWith({ stage: "installing", percent: 100 });
  });

  it("clears the Windows relaunch marker when installation fails in place", async () => {
    const installError = new Error("installer rejected the payload");
    const close = vi.fn(async () => {});
    const downloadAndInstall = vi.fn(async () => {
      throw installError;
    });
    setPendingAppUpdateForTests({
      currentVersion: "1.3.1",
      version: "1.3.2",
      close,
      downloadAndInstall,
    });
    const prepare = vi.spyOn(ipc, "prepareUpdateRelaunch").mockResolvedValue();
    const cancel = vi.spyOn(ipc, "cancelUpdateRelaunch").mockResolvedValue();

    await expect(installAppUpdate(vi.fn())).rejects.toBe(installError);

    expect(prepare).toHaveBeenCalledOnce();
    expect(cancel).toHaveBeenCalledOnce();
    expect(downloadAndInstall).toHaveBeenCalledOnce();
    expect(close).not.toHaveBeenCalled();
  });

  it("does not download when native relaunch preparation fails", async () => {
    const preparationError = new Error("could not persist restart intent");
    const close = vi.fn(async () => {});
    const downloadAndInstall = vi.fn(async () => {});
    setPendingAppUpdateForTests({
      currentVersion: "1.3.1",
      version: "1.3.2",
      close,
      downloadAndInstall,
    });
    vi.spyOn(ipc, "prepareUpdateRelaunch").mockRejectedValue(preparationError);
    const cancel = vi.spyOn(ipc, "cancelUpdateRelaunch").mockResolvedValue();

    await expect(installAppUpdate(vi.fn())).rejects.toBe(preparationError);

    expect(downloadAndInstall).not.toHaveBeenCalled();
    expect(cancel).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
  });

  it("retries only relaunch after an installed update fails its readiness handshake", async () => {
    const close = vi.fn(async () => {});
    const downloadAndInstall = vi.fn(async () => {});
    setPendingAppUpdateForTests({
      currentVersion: "1.3.1",
      version: "1.3.2",
      close,
      downloadAndInstall,
    });
    const prepare = vi.spyOn(ipc, "prepareUpdateRelaunch").mockResolvedValue();
    const relaunch = vi
      .spyOn(ipc, "relaunchAfterUpdate")
      .mockRejectedValueOnce(new Error("replacement did not become ready"))
      .mockResolvedValueOnce();
    const onProgress = vi.fn();

    await expect(installAppUpdate(onProgress)).rejects.toThrow(
      "replacement did not become ready",
    );
    await installAppUpdate(onProgress);

    expect(prepare).toHaveBeenCalledOnce();
    expect(downloadAndInstall).toHaveBeenCalledOnce();
    expect(close).toHaveBeenCalledOnce();
    expect(relaunch).toHaveBeenCalledTimes(2);
    expect(onProgress).toHaveBeenLastCalledWith({ stage: "installing", percent: 100 });
  });
});
