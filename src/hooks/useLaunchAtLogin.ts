import { useCallback, useEffect, useState } from "react";

import { isTauri } from "../lib/ipc";

export interface LaunchAtLoginState {
  available: boolean;
  enabled: boolean;
  busy: boolean;
  error: string | null;
  setEnabled: (enabled: boolean) => Promise<void>;
}

function message(cause: unknown): string {
  return cause instanceof Error && cause.message.trim()
    ? cause.message
    : "EyeUrAI could not change your login item.";
}

/** Mirrors the macOS login-item state; the OS remains the source of truth. */
export function useLaunchAtLogin(): LaunchAtLoginState {
  const available = isTauri();
  const [enabled, setEnabledState] = useState(false);
  const [busy, setBusy] = useState(available);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!available) return;
    let active = true;
    void import("@tauri-apps/plugin-autostart")
      .then(({ isEnabled }) => isEnabled())
      .then((value) => {
        if (active) setEnabledState(value);
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
  }, [available]);

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
