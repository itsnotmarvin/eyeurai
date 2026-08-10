import { useEffect, useRef, useState } from "react";

import {
  PROVIDER_META,
  PROVIDER_ORDER,
  type Account,
  type DisconnectedAccount,
  type Preferences,
  type ProviderId,
} from "../types/quota";
import { reconcileThresholds } from "../lib/preferences";
import { displayPercent, menuBarQuotaLabel } from "../lib/format";
import { ensureNotificationPermission } from "../lib/notify";
import {
  startCodexAccountLogin,
  subscribeToCodexLogin,
  type CodexLoginEvent,
} from "../lib/ipc";
import { ProviderMark } from "./ProviderMark";
import { Switch } from "./controls/Switch";
import { ThresholdSlider } from "./controls/ThresholdSlider";

export interface SettingsViewProps {
  preferences: Preferences;
  onChange: (next: Preferences) => void;
  /** "demo" when running outside Tauri with sample data. */
  mode: "live" | "demo";
  onRerunSetup: () => void;
  accounts: Account[];
  onDisconnectAccount: (account: Account) => void;
  onReconnectAccount: (account: DisconnectedAccount) => void;
  onRefreshAccounts: () => void;
  onRequestLocalUsage: () => void;
  appVersion?: string;
  requestPermission?: () => Promise<boolean>;
}

const SHORTCUTS: Array<[string, string]> = [
  ["R", "Refresh now"],
  [",", "Open settings"],
  ["Esc", "Back / hide popover"],
];

const CONNECTION_COPY: Record<
  ProviderId,
  { title: string; body: string; command?: string; note?: string }
> = {
  claude: {
    title: "Connect Claude",
    body: "Run Claude Code sign-in in a terminal, then check again. EyeUrAI observes whichever Claude account that CLI currently uses.",
    command: "claude /login",
    note: "Previously retained account identities stay listed when the terminal login changes. Claude controls whether separate CLI logins can coexist.",
  },
  openai: {
    title: "Connect OpenAI / Codex",
    body: "EyeUrAI creates a separate Codex profile and opens OpenAI's official browser sign-in. Each profile stays independently connected, so adding another account does not replace this one.",
    note: "Codex owns the OAuth flow, credential files, and token refresh inside each isolated profile. EyeUrAI only reads account and rate-limit metadata from the official Codex app-server.",
  },
  openrouter: {
    title: "Connect OpenRouter",
    body: "For this first version, launch EyeUrAI with an OPENROUTER_API_KEY environment variable. Secure in-app key storage is the next connection milestone.",
    command: "OPENROUTER_API_KEY",
  },
  gemini: {
    title: "Connect Gemini",
    body: "Gemini does not currently expose a safe account-wide subscription usage percentage that EyeUrAI can read. We will enable this as soon as a truthful provider connection is available.",
  },
};

function sourceLabel(account: Account): string {
  if (account.source === "oauth") return "Existing provider login";
  if (account.source === "cli") return "Existing CLI login";
  if (account.source === "api-key") return "API key";
  return "Local connection";
}

function accountSourceLabel(account: Account): string {
  const plan = account.plan ? ` · ${account.plan}` : "";
  if (account.provider === "openai" && account.id.startsWith("codex-profile:")) {
    const state = account.status === "fresh" ? "Live" : "Last read unavailable";
    return `Connected Codex profile · ${state}${plan}`;
  }
  if (account.provider === "claude" || account.provider === "openai") {
    if (account.isCliActive === true) return `Current terminal login${plan}`;
    if (account.isCliActive === false) return `Retained account · Last known data${plan}`;
  }
  return `${sourceLabel(account)}${plan}`;
}

export function SettingsView({
  preferences,
  onChange,
  mode,
  onRerunSetup,
  accounts,
  onDisconnectAccount,
  onReconnectAccount,
  onRefreshAccounts,
  onRequestLocalUsage,
  appVersion = "1.0.0",
  requestPermission,
}: SettingsViewProps) {
  const [permissionDenied, setPermissionDenied] = useState(false);
  const [connectOpen, setConnectOpen] = useState(false);
  const [connectProvider, setConnectProvider] = useState<ProviderId | null>(null);
  const [codexLogin, setCodexLogin] = useState<{
    status: "idle" | "starting" | "waiting" | "success" | "error";
    profileId?: string;
    message?: string;
  }>({ status: "idle" });
  const codexLoginAttemptId = useRef(0);
  const codexLoginMounted = useRef(true);
  const codexLoginAttempt = useRef<{
    id: number;
    profileId?: string;
    pendingEvents: Map<string, CodexLoginEvent>;
  } | null>(null);
  const codexLoginListener = useRef<((event: CodexLoginEvent) => void) | null>(null);
  const codexLoginSubscription = useRef<Promise<() => void> | null>(null);
  const askForPermission = requestPermission ?? ensureNotificationPermission;
  const connectedAccounts = accounts.filter((account) => !/-status-\d+$/.test(account.id));
  const pinnedAccount = preferences.pinnedQuota
    ? connectedAccounts.find((account) => account.id === preferences.pinnedQuota?.accountId)
    : null;
  const pinnedWindow = preferences.pinnedQuota
    ? pinnedAccount?.windows.find((window) => window.id === preferences.pinnedQuota?.windowId)
    : null;
  const pinnedDisplay = pinnedWindow
    ? `${menuBarQuotaLabel(pinnedWindow)}:${displayPercent(pinnedWindow.percentUsed)}%`
    : preferences.pinnedQuota
      ? "Unavailable"
      : null;

  function completeCodexLogin(
    attempt: NonNullable<typeof codexLoginAttempt.current>,
    event: CodexLoginEvent,
  ): void {
    if (codexLoginAttempt.current?.id !== attempt.id || attempt.profileId !== event.profileId) {
      return;
    }
    codexLoginAttempt.current = null;
    setCodexLogin(
      event.success
        ? { status: "success", profileId: event.profileId }
        : {
            status: "error",
            profileId: event.profileId,
            message: event.message ?? "Codex sign-in was not completed.",
          },
    );
    if (event.success) onRefreshAccounts();
  }

  codexLoginListener.current = (event) => {
    if (!codexLoginMounted.current) return;
    const attempt = codexLoginAttempt.current;
    if (!attempt) return;
    if (!attempt.profileId) {
      // A completion can beat the start command's response. Keep a small,
      // profile-keyed buffer until that response provides the correlation ID.
      attempt.pendingEvents.set(event.profileId, event);
      if (attempt.pendingEvents.size > 16) {
        const oldestProfileId = attempt.pendingEvents.keys().next().value;
        if (oldestProfileId !== undefined) attempt.pendingEvents.delete(oldestProfileId);
      }
      return;
    }
    completeCodexLogin(attempt, event);
  };

  function ensureCodexLoginSubscription(): Promise<() => void> {
    if (!codexLoginSubscription.current) {
      codexLoginSubscription.current = subscribeToCodexLogin((event) => {
        codexLoginListener.current?.(event);
      });
    }
    return codexLoginSubscription.current;
  }

  useEffect(() => {
    codexLoginMounted.current = true;
    const subscription = ensureCodexLoginSubscription();
    return () => {
      codexLoginMounted.current = false;
      codexLoginAttempt.current = null;
      if (codexLoginSubscription.current === subscription) {
        codexLoginSubscription.current = null;
      }
      void subscription.then((unlisten) => unlisten());
    };
  }, []);

  async function beginCodexLogin(): Promise<void> {
    const attempt: NonNullable<typeof codexLoginAttempt.current> = {
      id: ++codexLoginAttemptId.current,
      pendingEvents: new Map<string, CodexLoginEvent>(),
    };
    codexLoginAttempt.current = attempt;
    setCodexLogin({ status: "starting" });
    try {
      // Do not open the browser flow until its completion listener is active.
      await ensureCodexLoginSubscription();
      if (codexLoginAttempt.current?.id !== attempt.id) return;
      const started = await startCodexAccountLogin();
      if (codexLoginAttempt.current?.id !== attempt.id) return;
      if (!started) {
        codexLoginAttempt.current = null;
        setCodexLogin({
          status: "error",
          message: "Codex account sign-in is available in the installed EyeUrAI app.",
        });
        return;
      }
      attempt.profileId = started.profileId;
      const earlyCompletion = attempt.pendingEvents.get(started.profileId);
      attempt.pendingEvents.clear();
      if (earlyCompletion) {
        completeCodexLogin(attempt, earlyCompletion);
      } else {
        setCodexLogin({ status: "waiting", profileId: started.profileId });
      }
    } catch (cause) {
      if (codexLoginAttempt.current?.id !== attempt.id) return;
      codexLoginAttempt.current = null;
      setCodexLogin({
        status: "error",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }

  function toggleProvider(provider: ProviderId, enabled: boolean): void {
    const next = enabled
      ? [...preferences.enabledProviders, provider]
      : preferences.enabledProviders.filter((id) => id !== provider);
    onChange({
      ...preferences,
      enabledProviders: PROVIDER_ORDER.filter((id) => next.includes(id)),
    });
  }

  async function toggleNotifications(enabled: boolean): Promise<void> {
    if (!enabled) {
      setPermissionDenied(false);
      onChange({ ...preferences, notificationsEnabled: false });
      return;
    }
    const granted = await askForPermission();
    setPermissionDenied(!granted);
    onChange({ ...preferences, notificationsEnabled: granted });
  }

  return (
    <div className="settings" role="region" aria-label="Settings">
      <section className="settings__section">
        <div className="settings__headingrow">
          <h2 className="settings__title">Accounts</h2>
          <button
            type="button"
            className="btn btn--ghost btn--mini"
            onClick={() => {
              setConnectProvider(null);
              setCodexLogin({ status: "idle" });
              setConnectOpen(true);
            }}
          >
            <span aria-hidden="true">+</span> Add account
          </button>
        </div>
        <p className="settings__caption">
          EyeUrAI found existing sign-ins on this Mac. It did not sign in for you.
        </p>
        <div className="settings__accounts">
          {connectedAccounts.map((account) => (
            <div className="settings__account" data-provider={account.provider} key={account.id}>
              <span className="settings__providerMark">
                <ProviderMark provider={account.provider} size={15} />
              </span>
              <span className="settings__accountText">
                <span className="settings__accountName">{account.label}</span>
                <span className="settings__accountSource">
                  {accountSourceLabel(account)}
                </span>
              </span>
              <button
                type="button"
                className="settings__disconnect"
                aria-label={`Disconnect ${PROVIDER_META[account.provider].name} account ${account.label} from EyeUrAI`}
                onClick={() => onDisconnectAccount(account)}
              >
                Disconnect
              </button>
            </div>
          ))}
          {preferences.disconnectedAccounts.map((account) => (
            <div
              className="settings__account settings__account--disconnected"
              data-provider={account.provider}
              key={`disconnected-${account.id}`}
            >
              <span className="settings__providerMark">
                <ProviderMark provider={account.provider} size={15} />
              </span>
              <span className="settings__accountText">
                <span className="settings__accountName">{account.label}</span>
                <span className="settings__accountSource">Disconnected from EyeUrAI</span>
              </span>
              <button
                type="button"
                className="settings__reconnect"
                aria-label={`Reconnect ${PROVIDER_META[account.provider].name} account ${account.label} to EyeUrAI`}
                onClick={() => onReconnectAccount(account)}
              >
                Reconnect
              </button>
            </div>
          ))}
          {connectedAccounts.length === 0 && preferences.disconnectedAccounts.length === 0 ? (
            <p className="settings__accountEmpty">No local provider sign-ins detected yet.</p>
          ) : null}
        </div>
        <p className="settings__privacy">
          Disconnect hides retained quota data for that recognized account from EyeUrAI views and
          alerts. It does not log you out or delete provider credentials.
        </p>
      </section>

      <section className="settings__section">
        <h2 className="settings__title">Providers</h2>
        <p className="settings__caption">Hide the ones you do not use.</p>
        <div className="settings__list">
          {PROVIDER_ORDER.map((provider) => {
            const meta = PROVIDER_META[provider];
            return (
              <div className="settings__provider" data-provider={provider} key={provider}>
                <span className="settings__providerMark">
                  <ProviderMark provider={provider} size={15} />
                </span>
                <Switch
                  label={meta.name}
                  description={meta.blurb}
                  checked={preferences.enabledProviders.includes(provider)}
                  onChange={(checked) => toggleProvider(provider, checked)}
                />
              </div>
            );
          })}
        </div>
        <div className="settings__usageAccess">
          <Switch
            label="Local token usage"
            description="Aggregate Claude Code and Codex token counters from this computer."
            checked={preferences.localUsageEnabled}
            onChange={(enabled) => {
              if (enabled) onRequestLocalUsage();
              else onChange({ ...preferences, localUsageEnabled: false });
            }}
          />
          <p className="settings__privacy">
            Optional and read-only. EyeUrAI asks before scanning local session files.
          </p>
        </div>
      </section>

      <section className="settings__section">
        <h2 className="settings__title">Alerts</h2>
        <Switch
          label="Desktop notifications"
          description="One alert per bar, each time it crosses a threshold."
          checked={preferences.notificationsEnabled}
          onChange={(value) => void toggleNotifications(value)}
        />
        {permissionDenied ? (
          <p className="settings__warning" role="status">
            Your system denied EyeUrAI notifications. Enable them in system notification settings.
          </p>
        ) : null}
        <ThresholdSlider
          label="Warn at"
          tone="warn"
          value={preferences.warnThreshold}
          onChange={(value) =>
            onChange(reconcileThresholds({ ...preferences, warnThreshold: value }, "warn"))
          }
        />
        <ThresholdSlider
          label="Critical at"
          tone="critical"
          value={preferences.criticalThreshold}
          onChange={(value) =>
            onChange(reconcileThresholds({ ...preferences, criticalThreshold: value }, "critical"))
          }
        />
      </section>

      <section className="settings__section">
        <h2 className="settings__title">Appearance</h2>
        <Switch
          label="Compact rows"
          description="Tighter spacing when you track many accounts."
          checked={preferences.compact}
          onChange={(value) => onChange({ ...preferences, compact: value })}
        />
        <div className="settings__trayChoice">
          <div>
            <span className="settings__trayLabel">Menu bar display</span>
            <span className="settings__trayDescription">
              {pinnedWindow && pinnedAccount
                ? `${PROVIDER_META[pinnedAccount.provider].name} · ${pinnedAccount.label} · ${pinnedWindow.label}`
                : preferences.pinnedQuota
                  ? "The pinned quota is temporarily unavailable; the logo is showing."
                  : "Click any quota on Home to pin its live usage."}
            </span>
          </div>
          <div className="settings__trayOptions" aria-label="Menu bar display">
            <button
              type="button"
              aria-pressed={preferences.pinnedQuota === null}
              onClick={() => onChange({ ...preferences, pinnedQuota: null })}
            >
              Logo
            </button>
            {pinnedDisplay ? (
              <button
                type="button"
                aria-pressed="true"
                aria-label={`${pinnedDisplay}. Unpin quota and restore EyeUrAI logo`}
                onClick={() => onChange({ ...preferences, pinnedQuota: null })}
              >
                {pinnedDisplay}
              </button>
            ) : null}
          </div>
        </div>
      </section>

      <section className="settings__section">
        <h2 className="settings__title">Keyboard</h2>
        <ul className="shortcuts">
          {SHORTCUTS.map(([key, description]) => (
            <li className="shortcuts__row" key={key}>
              <kbd className="kbd">{key}</kbd>
              <span>{description}</span>
            </li>
          ))}
        </ul>
      </section>

      <section className="settings__section">
        <h2 className="settings__title">About</h2>
        <p className="settings__caption">
          EyeUrAI v{appVersion} · local-first and open source. Quotas are read on this computer;
          credentials stay in provider or operating-system storage and never enter the interface.
        </p>
        <div className="settings__badges">
          <span className="badge" data-mode={mode}>
            {mode === "live" ? "Connected to local agent" : "Preview data"}
          </span>
        </div>
        <button type="button" className="btn btn--ghost btn--block" onClick={onRerunSetup}>
          Run first-time setup again
        </button>
      </section>

      {connectOpen ? (
        <div className="connectsheet" role="dialog" aria-modal="true" aria-label="Add account">
          <div className="connectsheet__card">
            <div className="connectsheet__head">
              <div>
                <h2 className="connectsheet__title">
                  {connectProvider ? CONNECTION_COPY[connectProvider].title : "Add an account"}
                </h2>
                <p className="connectsheet__subtitle">
                  {connectProvider
                    ? "EyeUrAI retains non-secret identity details; credentials stay in provider-managed or operating-system storage."
                    : "Choose a provider. OpenAI can add an isolated Codex account; other providers check their existing sign-ins."}
                </p>
              </div>
              <button
                type="button"
                className="connectsheet__close"
                aria-label="Close add account"
                onClick={() => setConnectOpen(false)}
              >
                ×
              </button>
            </div>

            {connectProvider ? (
              <div className="connectsheet__detail" data-provider={connectProvider}>
                <span className="connectsheet__mark">
                  <ProviderMark provider={connectProvider} size={22} />
                </span>
                <p>{CONNECTION_COPY[connectProvider].body}</p>
                {CONNECTION_COPY[connectProvider].command ? (
                  <code className="connectsheet__command">
                    {CONNECTION_COPY[connectProvider].command}
                  </code>
                ) : null}
                {CONNECTION_COPY[connectProvider].note ? (
                  <p className="connectsheet__note">{CONNECTION_COPY[connectProvider].note}</p>
                ) : null}
                {connectProvider === "openai" && codexLogin.status !== "idle" ? (
                  <p
                    className="connectsheet__note"
                    role={codexLogin.status === "error" ? "alert" : "status"}
                  >
                    {codexLogin.status === "starting"
                      ? "Starting the Codex sign-in service…"
                      : codexLogin.status === "waiting"
                        ? "Your browser is open. Finish signing into the Codex account there."
                        : codexLogin.status === "success"
                          ? "Codex account added. Its usage can now refresh independently."
                          : codexLogin.message}
                  </p>
                ) : null}
              </div>
            ) : (
              <div className="connectsheet__providers">
                {PROVIDER_ORDER.map((provider) => (
                  <button
                    type="button"
                    className="connectsheet__provider"
                    data-provider={provider}
                    key={provider}
                    onClick={() => {
                      setConnectProvider(provider);
                      if (provider === "openai") setCodexLogin({ status: "idle" });
                    }}
                  >
                    <span className="connectsheet__mark">
                      <ProviderMark provider={provider} size={18} />
                    </span>
                    <span>{PROVIDER_META[provider].name}</span>
                    <span aria-hidden="true">›</span>
                  </button>
                ))}
              </div>
            )}

            <div className="connectsheet__actions">
              {connectProvider ? (
                <button
                  type="button"
                  className="btn btn--ghost"
                  onClick={() => setConnectProvider(null)}
                >
                  Back
                </button>
              ) : null}
              {connectProvider === "openai" ? (
                <button
                  type="button"
                  className="btn btn--primary"
                  disabled={codexLogin.status === "starting" || codexLogin.status === "waiting"}
                  onClick={() => {
                    if (codexLogin.status === "success") {
                      setConnectOpen(false);
                    } else {
                      void beginCodexLogin();
                    }
                  }}
                >
                  {codexLogin.status === "starting"
                    ? "Starting…"
                    : codexLogin.status === "waiting"
                      ? "Waiting for browser…"
                      : codexLogin.status === "success"
                        ? "Done"
                        : codexLogin.status === "error"
                          ? "Try again"
                          : "Add Codex account"}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn btn--primary"
                  onClick={() => {
                    onRefreshAccounts();
                    setConnectOpen(false);
                  }}
                >
                  Check again
                </button>
              )}
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
