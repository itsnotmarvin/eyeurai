// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoSnapshot } from "../lib/demo";
import * as ipc from "../lib/ipc";
import { useSnapshot } from "./useSnapshot";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("useSnapshot", () => {
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
});
