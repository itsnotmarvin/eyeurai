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

/** Background poll interval while the popover is open. */
const POLL_INTERVAL_MS = 45_000;

function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Could not read quota data.";
}

export function useSnapshot(excludedAccountIds: readonly string[] = []): SnapshotController {
  const live = isTauri();
  const exclusionsKey = [...excludedAccountIds].sort().join("\u0000");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [phase, setPhase] = useState<SnapshotPhase>("loading");
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const mounted = useRef(true);
  const inFlight = useRef(false);
  const snapshotRef = useRef<Snapshot | null>(null);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
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
    async (options: { manual: boolean }) => {
      if (inFlight.current) return;
      inFlight.current = true;
      if (options.manual && mounted.current) setRefreshing(true);

      try {
        if (!live) {
          const previous = snapshotRef.current;
          // Background polls must not undo demo drift; only manual refreshes move.
          if (!options.manual) {
            if (!previous) apply(createDemoSnapshot(Date.now()));
            return;
          }
          // Keep a visible beat so the refresh affordance reads as real.
          await new Promise((resolve) => setTimeout(resolve, 420));
          apply(
            previous ? refreshDemoSnapshot(previous, Date.now()) : createDemoSnapshot(Date.now()),
          );
          return;
        }

        const exclusions = exclusionsKey ? exclusionsKey.split("\u0000") : [];
        const next = options.manual
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
        if (mounted.current) setRefreshing(false);
      }
    },
    [apply, exclusionsKey, live],
  );

  useEffect(() => {
    void load({ manual: false });
    const interval = setInterval(() => void load({ manual: false }), POLL_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [load]);

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
    void subscribeToRefreshRequests(() => void load({ manual: true })).then((unlisten) => {
      if (cancelled) unlisten();
      else dispose = unlisten;
    });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [live, load]);

  const refresh = useCallback(() => {
    void load({ manual: true });
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
