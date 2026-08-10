import { useSyncExternalStore } from "react";

/**
 * A single shared 1s clock for every countdown in the popover, so N bars do not
 * each spin up their own interval. The timer only runs while something is
 * subscribed, which keeps the app idle when the window is hidden.
 */

type Listener = () => void;

const listeners = new Set<Listener>();
let current = Date.now();
let timer: ReturnType<typeof setInterval> | null = null;

function tick(): void {
  current = Date.now();
  for (const listener of listeners) listener();
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  if (timer === null) {
    current = Date.now();
    timer = setInterval(tick, 1000);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}

function getSnapshot(): number {
  return current;
}

/** Current wall-clock time in ms, refreshed once per second. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
