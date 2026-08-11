import { useEffect, useRef, useState } from "react";

import type { ProfileLoginEvent, ProfileLoginStarted } from "../lib/ipc";

export interface ProfileLoginState {
  status: "idle" | "starting" | "waiting" | "success" | "error";
  profileId?: string;
  message?: string;
}

export interface ProfileLoginOptions {
  /** Backend command that opens the provider's browser sign-in. */
  start: () => Promise<ProfileLoginStarted | null>;
  /** Subscription to that sign-in's completion events. */
  subscribe: (onEvent: (event: ProfileLoginEvent) => void) => Promise<() => void>;
  /** Shown when the backend command is unavailable (browser preview). */
  unavailableMessage: string;
  /** Shown when a completion event arrives without a message. */
  incompleteMessage: string;
  /** Called once per successful sign-in, e.g. to refresh accounts. */
  onSuccess: () => void;
}

/**
 * One provider profile sign-in flow: start the browser flow, correlate its
 * completion event by profile id, and surface a small status machine.
 *
 * A completion event can beat the start command's response, so events are
 * buffered per profile id until the response provides the correlation id.
 */
export function useProfileLogin(options: ProfileLoginOptions): {
  login: ProfileLoginState;
  begin: () => Promise<void>;
  reset: () => void;
} {
  const [login, setLogin] = useState<ProfileLoginState>({ status: "idle" });
  const attemptId = useRef(0);
  const mounted = useRef(true);
  const attempt = useRef<{
    id: number;
    profileId?: string;
    pendingEvents: Map<string, ProfileLoginEvent>;
  } | null>(null);
  const listener = useRef<((event: ProfileLoginEvent) => void) | null>(null);
  const subscription = useRef<Promise<() => void> | null>(null);
  const optionsRef = useRef(options);
  optionsRef.current = options;

  function complete(
    current: NonNullable<typeof attempt.current>,
    event: ProfileLoginEvent,
  ): void {
    if (attempt.current?.id !== current.id || current.profileId !== event.profileId) {
      return;
    }
    attempt.current = null;
    setLogin(
      event.success
        ? { status: "success", profileId: event.profileId }
        : {
            status: "error",
            profileId: event.profileId,
            message: event.message ?? optionsRef.current.incompleteMessage,
          },
    );
    if (event.success) optionsRef.current.onSuccess();
  }

  listener.current = (event) => {
    if (!mounted.current) return;
    const current = attempt.current;
    if (!current) return;
    if (!current.profileId) {
      current.pendingEvents.set(event.profileId, event);
      if (current.pendingEvents.size > 16) {
        const oldestProfileId = current.pendingEvents.keys().next().value;
        if (oldestProfileId !== undefined) current.pendingEvents.delete(oldestProfileId);
      }
      return;
    }
    complete(current, event);
  };

  function ensureSubscription(): Promise<() => void> {
    if (!subscription.current) {
      subscription.current = optionsRef.current.subscribe((event) => {
        listener.current?.(event);
      });
    }
    return subscription.current;
  }

  useEffect(() => {
    mounted.current = true;
    const active = ensureSubscription();
    return () => {
      mounted.current = false;
      attempt.current = null;
      if (subscription.current === active) {
        subscription.current = null;
      }
      void active.then((unlisten) => unlisten());
    };
    // The subscription is provider-fixed for the life of the component.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function begin(): Promise<void> {
    const current: NonNullable<typeof attempt.current> = {
      id: ++attemptId.current,
      pendingEvents: new Map<string, ProfileLoginEvent>(),
    };
    attempt.current = current;
    setLogin({ status: "starting" });
    try {
      // Do not open the browser flow until its completion listener is active.
      await ensureSubscription();
      if (attempt.current?.id !== current.id) return;
      const started = await optionsRef.current.start();
      if (attempt.current?.id !== current.id) return;
      if (!started) {
        attempt.current = null;
        setLogin({ status: "error", message: optionsRef.current.unavailableMessage });
        return;
      }
      current.profileId = started.profileId;
      const earlyCompletion = current.pendingEvents.get(started.profileId);
      current.pendingEvents.clear();
      if (earlyCompletion) {
        complete(current, earlyCompletion);
      } else {
        setLogin({ status: "waiting", profileId: started.profileId });
      }
    } catch (cause) {
      if (attempt.current?.id !== current.id) return;
      attempt.current = null;
      setLogin({
        status: "error",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }

  function reset(): void {
    attempt.current = null;
    setLogin({ status: "idle" });
  }

  return { login, begin, reset };
}
