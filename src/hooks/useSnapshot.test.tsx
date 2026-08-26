// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoSnapshot } from "../lib/demo";
import * as ipc from "../lib/ipc";
import { resolveSnapshotMode, useSnapshot } from "./useSnapshot";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("useSnapshot", () => {
  it("never selects demo data for a production browser build", () => {
    expect(resolveSnapshotMode(false, false)).toBe("browser");
    expect(resolveSnapshotMode(false, true)).toBe("demo");
    expect(resolveSnapshotMode(true, false)).toBe("live");
  });

  it("queues one manual refresh behind an in-flight load", async () => {
    let resolveInitial!: (snapshot: ReturnType<typeof createDemoSnapshot>) => void;
    const initialResult = new Promise<ReturnType<typeof createDemoSnapshot>>((resolve) => {
      resolveInitial = resolve;
    });
    vi.spyOn(ipc, "isTauri").mockReturnValue(true);
    const fetchSnapshot = vi.spyOn(ipc, "fetchSnapshot").mockReturnValue(initialResult);
    const requestRefresh = vi
      .spyOn(ipc, "requestRefresh")
      .mockResolvedValue(createDemoSnapshot(Date.now() + 1_000));
    vi.spyOn(ipc, "subscribeToSnapshots").mockResolvedValue(() => {});
    vi.spyOn(ipc, "subscribeToRefreshRequests").mockResolvedValue(() => {});

    const { result } = renderHook(() => useSnapshot());
    await waitFor(() => expect(fetchSnapshot).toHaveBeenCalledTimes(1));

    act(() => {
      result.current.refresh();
      result.current.refresh();
    });
    expect(requestRefresh).not.toHaveBeenCalled();

    await act(async () => {
      resolveInitial(createDemoSnapshot());
      await initialResult;
    });
    await waitFor(() => expect(requestRefresh).toHaveBeenCalledTimes(1));
  });

  it("automatically re-reads live provider usage without showing a manual refresh", async () => {
    const intervalMs = 15_000;
    vi.useFakeTimers();
    vi.spyOn(ipc, "isTauri").mockReturnValue(true);
    const fetchSnapshot = vi
      .spyOn(ipc, "fetchSnapshot")
      .mockResolvedValue(createDemoSnapshot());
    const requestRefresh = vi
      .spyOn(ipc, "requestRefresh")
      .mockResolvedValue(createDemoSnapshot(Date.now() + intervalMs));
    vi.spyOn(ipc, "subscribeToSnapshots").mockResolvedValue(() => {});
    vi.spyOn(ipc, "subscribeToRefreshRequests").mockResolvedValue(() => {});

    const { result } = renderHook(() => useSnapshot([], intervalMs));
    await act(async () => {
      await Promise.resolve();
    });

    expect(fetchSnapshot).toHaveBeenCalledTimes(1);
    expect(requestRefresh).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(intervalMs);
    });

    expect(requestRefresh).toHaveBeenCalledTimes(1);
    expect(result.current.refreshing).toBe(false);
  });

  it("shows an error instead of fabricating demo accounts when live IPC returns nothing", async () => {
    vi.spyOn(ipc, "isTauri").mockReturnValue(true);
    vi.spyOn(ipc, "fetchSnapshot").mockResolvedValue(null);
    vi.spyOn(ipc, "subscribeToSnapshots").mockResolvedValue(() => {});
    vi.spyOn(ipc, "subscribeToRefreshRequests").mockResolvedValue(() => {});

    const { result } = renderHook(() => useSnapshot());
    await waitFor(() => expect(result.current.phase).toBe("error"));

    expect(result.current.mode).toBe("live");
    expect(result.current.snapshot).toBeNull();
    expect(result.current.error).toMatch(/local agent/i);
  });
});
