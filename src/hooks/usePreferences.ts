import { useCallback, useEffect, useRef, useState } from "react";

import type { Preferences } from "../types/quota";
import { loadPreferences, sanitizePreferences, savePreferences } from "../lib/preferences";

export interface PreferencesController {
  preferences: Preferences;
  /** Applies a partial patch and persists the sanitised result. */
  update: (patch: Partial<Preferences>) => void;
  /** Replaces the whole object (used by onboarding and reset). */
  replace: (next: Preferences) => void;
}

function loadInitialPreferences(): Preferences {
  const preferences = loadPreferences();
  if (
    import.meta.env.DEV &&
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).has("preview-update")
  ) {
    return { ...preferences, onboardingCompleted: true };
  }
  return preferences;
}

export function usePreferences(): PreferencesController {
  const [preferences, setPreferences] = useState<Preferences>(loadInitialPreferences);
  const firstRun = useRef(true);

  useEffect(() => {
    // Skip the initial write so a read-only preview never dirties storage.
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    savePreferences(preferences);
  }, [preferences]);

  const update = useCallback((patch: Partial<Preferences>) => {
    setPreferences((current) => sanitizePreferences({ ...current, ...patch }));
  }, []);

  const replace = useCallback((next: Preferences) => {
    setPreferences(sanitizePreferences(next));
  }, []);

  return { preferences, update, replace };
}
