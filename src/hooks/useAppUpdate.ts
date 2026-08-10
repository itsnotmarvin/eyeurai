import { useCallback, useEffect, useRef, useState } from "react";

import {
  checkForAppUpdate,
  installAppUpdate,
  type AppUpdateInfo,
  type AppUpdateProgress,
} from "../lib/appUpdates";

const PERIODIC_CHECK_MS = 4 * 60 * 60 * 1_000;
const FOCUS_CHECK_COOLDOWN_MS = 15 * 60 * 1_000;

export interface AppUpdateState {
  info: AppUpdateInfo | null;
  checking: boolean;
  progress: AppUpdateProgress | null;
  error: string | null;
  checkNow: () => Promise<void>;
  install: () => Promise<void>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error && error.message.trim()
    ? error.message
    : "The update could not be installed. Please try again.";
}

export function useAppUpdate(): AppUpdateState {
  const [info, setInfo] = useState<AppUpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [progress, setProgress] = useState<AppUpdateProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const lastCheckAt = useRef(0);
  const checkInFlight = useRef<Promise<void> | null>(null);

  const checkNow = useCallback(async () => {
    if (checkInFlight.current || info) return checkInFlight.current ?? Promise.resolve();
    setChecking(true);
    const task = checkForAppUpdate()
      .then((next) => {
        lastCheckAt.current = Date.now();
        if (next) setInfo(next);
      })
      .catch(() => {
        // A background update check must never interfere with quota monitoring.
        // It will retry on the next focus or periodic check.
        lastCheckAt.current = Date.now();
      })
      .finally(() => {
        checkInFlight.current = null;
        setChecking(false);
      });
    checkInFlight.current = task;
    return task;
  }, [info]);

  useEffect(() => {
    void checkNow();
    const interval = window.setInterval(() => void checkNow(), PERIODIC_CHECK_MS);
    const onFocus = () => {
      if (Date.now() - lastCheckAt.current >= FOCUS_CHECK_COOLDOWN_MS) void checkNow();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", onFocus);
    };
  }, [checkNow]);

  const install = useCallback(async () => {
    if (!info || progress) return;
    setError(null);
    setProgress({ stage: "downloading", percent: null });
    try {
      await installAppUpdate(setProgress);
    } catch (cause) {
      setProgress(null);
      setError(errorMessage(cause));
    }
  }, [info, progress]);

  return { info, checking, progress, error, checkNow, install };
}
