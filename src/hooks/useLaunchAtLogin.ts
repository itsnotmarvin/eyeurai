import { useCallback, useEffect, useRef, useState } from "react";

import { isTauri } from "../lib/ipc";

export interface LaunchAtLoginState {
  available: boolean;
  enabled: boolean;
  busy: boolean;
  error: string | null;
  setEnabled: (enabled: boolean) => Promise<void>;
}

export interface LaunchAtLoginOptions {
  /** Enables the OS login item once, while preserving later user opt-out. */
  enableByDefault?: boolean;
  onDefaultApplied?: () => void;
}

function message(cause: unknown): string {
  return cause instanceof Error && cause.message.trim()
    ? cause.message
    : "EyeUrAI could not change your login item.";
}

/** Mirrors the macOS login-item state; the OS remains the source of truth. */
export function useLaunchAtLogin({
  enableByDefault = false,
  onDefaultApplied,
}: LaunchAtLoginOptions = {}): LaunchAtLoginState {
  const available = isTauri();
  const [enabled, setEnabledState] = useState(false);
  const [busy, setBusy] = useState(available);
  const [error, setError] = useState<string | null>(null);
  const defaultAppliedRef = useRef(onDefaultApplied);
  defaultAppliedRef.current = onDefaultApplied;

  useEffect(() => {
    if (!available) return;
    let active = true;
    void import("@tauri-apps/plugin-autostart")
      .then(async (autostart) => {
        let value = await autostart.isEnabled();
        if (!value && enableByDefault) {
          await autostart.enable();
          value = await autostart.isEnabled();
        }
        return value;
      })
      .then((value) => {
        if (!active) return;
        setEnabledState(value);
        if (enableByDefault && value) defaultAppliedRef.current?.();
      })
      .catch((cause) => {
        if (active) setError(message(cause));
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, [available, enableByDefault]);

  const setEnabled = useCallback(
    async (next: boolean) => {
      if (!available || busy) return;
      setBusy(true);
      setError(null);
      try {
        const autostart = await import("@tauri-apps/plugin-autostart");
        if (next) await autostart.enable();
        else await autostart.disable();
        setEnabledState(await autostart.isEnabled());
      } catch (cause) {
        setError(message(cause));
      } finally {
        setBusy(false);
      }
    },
    [available, busy],
  );

  return { available, enabled, busy, error, setEnabled };
}
