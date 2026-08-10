import {
  PROVIDER_META,
  type Preferences,
  type Severity,
  type Snapshot,
} from "../types/quota";
import { displayPercent, severityFor } from "./format";
import { isTauri } from "./ipc";

/**
 * Threshold alerts.
 *
 * `computeAlerts` is a pure function so the escalation logic can be tested
 * without a notification backend: an alert only fires when a bar *crosses* into
 * a higher severity, never on every poll, and the memory resets once usage
 * drops back down (e.g. after a quota window resets).
 */

export interface QuotaAlert {
  key: string;
  severity: Exclude<Severity, "normal">;
  title: string;
  body: string;
}

/** Highest severity seen per bar, keyed by `accountId::windowId`. */
export type AlertState = Readonly<Record<string, Severity>>;

const SEVERITY_RANK: Record<Severity, number> = { normal: 0, warn: 1, critical: 2 };

export function computeAlerts(
  snapshot: Snapshot,
  preferences: Preferences,
  previous: AlertState,
): { alerts: QuotaAlert[]; state: AlertState } {
  const state: Record<string, Severity> = {};
  const alerts: QuotaAlert[] = [];

  for (const account of snapshot.accounts) {
    if (!preferences.enabledProviders.includes(account.provider)) continue;
    if (account.status === "error") continue;

    for (const window of account.windows) {
      const key = `${account.id}::${window.id}`;
      const severity = severityFor(
        window.percentUsed,
        preferences.warnThreshold,
        preferences.criticalThreshold,
      );
      state[key] = severity;

      const before = previous[key] ?? "normal";
      if (severity !== "normal" && SEVERITY_RANK[severity] > SEVERITY_RANK[before]) {
        const percent = displayPercent(window.percentUsed);
        alerts.push({
          key,
          severity,
          title:
            severity === "critical"
              ? `${PROVIDER_META[account.provider].name} limit almost gone`
              : `${PROVIDER_META[account.provider].name} usage is high`,
          body: `${account.label} · ${window.label} at ${percent}%`,
        });
      }
    }
  }

  return { alerts, state };
}

type NotificationModule = {
  isPermissionGranted: () => Promise<boolean>;
  requestPermission: () => Promise<"granted" | "denied" | "default">;
  sendNotification: (options: { title: string; body?: string }) => void;
};

async function loadNotificationModule(): Promise<NotificationModule | null> {
  if (!isTauri()) return null;
  try {
    return (await import("@tauri-apps/plugin-notification")) as unknown as NotificationModule;
  } catch {
    return null;
  }
}

/** Asks the OS for permission. Returns true when notifications can be sent. */
export async function ensureNotificationPermission(): Promise<boolean> {
  const module = await loadNotificationModule();
  if (module) {
    try {
      if (await module.isPermissionGranted()) return true;
      return (await module.requestPermission()) === "granted";
    } catch {
      return false;
    }
  }

  // Browser preview fallback.
  if (typeof Notification === "undefined") return false;
  try {
    if (Notification.permission === "granted") return true;
    if (Notification.permission === "denied") return false;
    return (await Notification.requestPermission()) === "granted";
  } catch {
    return false;
  }
}

export async function deliverAlerts(alerts: readonly QuotaAlert[]): Promise<void> {
  if (alerts.length === 0) return;
  const module = await loadNotificationModule();
  if (module) {
    for (const alert of alerts) {
      try {
        module.sendNotification({ title: alert.title, body: alert.body });
      } catch {
        /* never let a notification failure break the UI */
      }
    }
    return;
  }

  if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
  for (const alert of alerts) {
    try {
      new Notification(alert.title, { body: alert.body });
    } catch {
      /* ignore */
    }
  }
}
