import { useCallback, useEffect, useRef, useState } from "react";

import type { Snapshot } from "../types/quota";
import { createDemoSnapshot, refreshDemoSnapshot } from "../lib/demo";
import {
  fetchSnapshot,
  isTauri,
  requestRefresh,
  subscribeToRefreshRequests,
  subscribeToSnapshots,
} from "../lib/ipc";

export type SnapshotPhase = "loading" | "ready" | "error";
export type SnapshotMode = "live" | "demo";

export interface SnapshotController {
  snapshot: Snapshot | null;
  phase: SnapshotPhase;
  mode: SnapshotMode;
  refreshing: boolean;
  error: string | null;
  refresh: () => void;
}

/** Re-read live provider limits once a minute, including while the popover is hidden. */
export const AUTO_REFRESH_INTERVAL_MS = 60_000;

interface LoadOptions {
  /** User-requested refreshes show the refresh affordance; automatic ones stay quiet. */
  manual: boolean;
  /** A live refresh contacts providers instead of only reading the in-memory snapshot. */
  liveRefresh: boolean;
}

function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Could not read quota data.";
}

export function useSnapshot(
  excludedAccountIds: readonly string[] = [],
  autoRefreshIntervalMs: number = AUTO_REFRESH_INTERVAL_MS,
): SnapshotController {
  const live = isTauri();
  const recording =
    !live &&
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).has("recording");
  const exclusionsKey = [...excludedAccountIds].sort().join("\u0000");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [phase, setPhase] = useState<SnapshotPhase>("loading");
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const mounted = useRef(true);
  const inFlight = useRef(false);
  const queuedLoad = useRef<LoadOptions | null>(null);
  const snapshotRef = useRef<Snapshot | null>(null);
  const loadRef = useRef<(options: LoadOptions) => Promise<void>>(async () => {});

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      queuedLoad.current = null;
    };
  }, []);

  const apply = useCallback((next: Snapshot) => {
    snapshotRef.current = next;
    if (!mounted.current) return;
    setSnapshot(next);
    setPhase("ready");
    setError(null);
  }, []);

  const load = useCallback(
    async (options: LoadOptions) => {
      if (inFlight.current) {
        // Coalesce overlapping requests into one follow-up load. Preserve the
        // strongest request so neither a manual nor live refresh is swallowed.
        queuedLoad.current = {
          manual: options.manual || queuedLoad.current?.manual === true,
          liveRefresh: options.liveRefresh || queuedLoad.current?.liveRefresh === true,
        };
        return;
      }
      inFlight.current = true;
      if (options.manual && mounted.current) setRefreshing(true);

      try {
        if (!live) {
          const previous = snapshotRef.current;
          // Background polls must not undo demo drift; only manual refreshes move.
          if (!options.manual) {
            if (!previous) apply(createDemoSnapshot(Date.now(), recording));
            return;
          }
          // Keep a visible beat so the refresh affordance reads as real.
          await new Promise((resolve) => setTimeout(resolve, 420));
          apply(
            previous
              ? refreshDemoSnapshot(previous, Date.now())
              : createDemoSnapshot(Date.now(), recording),
          );
          return;
        }

        const exclusions = exclusionsKey ? exclusionsKey.split("\u0000") : [];
        const next = options.liveRefresh
          ? await requestRefresh(exclusions)
          : await fetchSnapshot(exclusions);
        if (next) {
          apply(next);
        } else if (!snapshotRef.current) {
          apply(createDemoSnapshot(Date.now()));
        }
      } catch (cause) {
        if (!mounted.current) return;
        setError(describeError(cause));
        setPhase(snapshotRef.current ? "ready" : "error");
      } finally {
        inFlight.current = false;
        const queued = queuedLoad.current;
        queuedLoad.current = null;
        if (mounted.current && !queued?.manual) setRefreshing(false);
        if (mounted.current && queued) void loadRef.current(queued);
      }
    },
    [apply, exclusionsKey, live, recording],
  );
  loadRef.current = load;

  useEffect(() => {
    void load({ manual: false, liveRefresh: false });
    const interval = setInterval(
      () => void load({ manual: false, liveRefresh: true }),
      autoRefreshIntervalMs,
    );
    return () => clearInterval(interval);
  }, [autoRefreshIntervalMs, load]);

  useEffect(() => {
    if (!live) return;
    let dispose: (() => void) | null = null;
    let cancelled = false;

    void subscribeToSnapshots((next) => apply(next)).then((unlisten) => {
      if (cancelled) unlisten();
      else dispose = unlisten;
    });

    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [apply, live]);

  useEffect(() => {
    if (!live) return;
    let dispose: (() => void) | null = null;
    let cancelled = false;
    void subscribeToRefreshRequests(
      () => void load({ manual: true, liveRefresh: true }),
    ).then((unlisten) => {
      if (cancelled) unlisten();
      else dispose = unlisten;
    });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [live, load]);

  const refresh = useCallback(() => {
    void load({ manual: true, liveRefresh: true });
  }, [load]);

  return {
    snapshot,
    phase,
    mode: live ? "live" : "demo",
    refreshing: refreshing || snapshot?.refreshing === true,
    error,
    refresh,
  };
}
