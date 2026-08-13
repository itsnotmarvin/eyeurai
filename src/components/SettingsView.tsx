import { useState } from "react";

import {
  APPEARANCE_THEMES,
  BACKGROUND_STYLES,
  PROVIDER_META,
  PROVIDER_ORDER,
  type Account,
  type AppearanceTheme,
  type BackgroundStyle,
  type DisconnectedAccount,
  type Preferences,
  type ProviderId,
} from "../types/quota";
import { REFRESH_INTERVAL_OPTIONS, reconcileThresholds } from "../lib/preferences";
import {
  displayPercent,
  menuBarQuotaLabel,
  menuBarResetCountdown,
} from "../lib/format";
import { ensureNotificationPermission } from "../lib/notify";
import {
  startClaudeAccountLogin,
  startCodexAccountLogin,
  subscribeToClaudeLogin,
  subscribeToCodexLogin,
} from "../lib/ipc";
import { useProfileLogin } from "../hooks/useProfileLogin";
import type { LaunchAtLoginState } from "../hooks/useLaunchAtLogin";
import { ProviderMark } from "./ProviderMark";
import { Switch } from "./controls/Switch";
import { ThresholdSlider } from "./controls/ThresholdSlider";

export interface SettingsViewProps {
  preferences: Preferences;
  now?: number;
  launchAtLogin?: LaunchAtLoginState;
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

const LOGIN_FLOW_COPY: Record<
  "claude" | "openai",
  { starting: string; waiting: string; success: string; add: string }
> = {
  claude: {
    starting: "Starting the Claude sign-in…",
    waiting: "Your browser is open. Finish signing into the Claude account there.",
    success: "Claude account added. Its usage can now refresh independently.",
    add: "Add Claude account",
  },
  openai: {
    starting: "Starting the Codex sign-in service…",
    waiting: "Your browser is open. Finish signing into the Codex account there.",
    success: "Codex account added. Its usage can now refresh independently.",
    add: "Add Codex account",
  },
};

const SHORTCUTS: Array<[string, string]> = [
  ["R", "Refresh now"],
  [",", "Open settings"],
  ["Esc", "Back / hide popover"],
];

const THEME_OPTIONS: ReadonlyArray<{
  id: AppearanceTheme;
  name: string;
  description: string;
}> = [
  { id: "porcelain", name: "Porcelain", description: "Editorial & warm" },
  { id: "carbon", name: "Carbon", description: "Technical & sharp" },
  { id: "lichen", name: "Lichen", description: "Organic & tactile" },
  { id: "nocturne", name: "Nocturne", description: "Atmospheric & calm" },
];

const BACKGROUND_OPTIONS: ReadonlyArray<{
  id: BackgroundStyle;
  label: string;
}> = [
  { id: "solid", label: "Solid" },
  { id: "material", label: "Material" },
  { id: "gradient", label: "Gradient" },
  { id: "photo", label: "Photo" },
];

const CONNECTION_COPY: Record<
  ProviderId,
  { title: string; body: string; command?: string; note?: string }
> = {
  claude: {
    title: "Connect Claude",
    body: "EyeUrAI opens Anthropic's official browser sign-in in a separate profile for each account. Adding another account does not replace this one — or your terminal login.",
    note: "Your existing Claude Code terminal login is picked up automatically. Accounts added here use a read-only sign-in that can see usage but can never run Claude.",
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
  if (account.provider === "claude" && account.id.startsWith("claude-profile:")) {
    const state = account.status === "fresh" ? "Live" : "Last read unavailable";
    return `Connected Claude account · ${state}${plan}`;
  }
  if (account.provider === "claude" || account.provider === "openai") {
    if (account.isCliActive === true) return `Current terminal login${plan}`;
    if (account.isCliActive === false) return `Retained account · Last known data${plan}`;
  }
  return `${sourceLabel(account)}${plan}`;
}

export function SettingsView({
  preferences,
  now = Date.now(),
  launchAtLogin,
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
  const codexFlow = useProfileLogin({
    start: startCodexAccountLogin,
    subscribe: subscribeToCodexLogin,
    unavailableMessage: "Codex account sign-in is available in the installed EyeUrAI app.",
    incompleteMessage: "Codex sign-in was not completed.",
    onSuccess: onRefreshAccounts,
  });
  const claudeFlow = useProfileLogin({
    start: startClaudeAccountLogin,
    subscribe: subscribeToClaudeLogin,
    unavailableMessage: "Claude account sign-in is available in the installed EyeUrAI app.",
    incompleteMessage: "Claude sign-in was not completed.",
    onSuccess: onRefreshAccounts,
  });
  const loginFlows = { openai: codexFlow, claude: claudeFlow } as const;
  const activeFlow =
    connectProvider === "openai" || connectProvider === "claude"
      ? loginFlows[connectProvider]
      : null;
  const askForPermission = requestPermission ?? ensureNotificationPermission;
  const connectedAccounts = accounts.filter((account) => !/-status-\d+$/.test(account.id));
  const pinnedAccount = preferences.pinnedQuota
    ? connectedAccounts.find((account) => account.id === preferences.pinnedQuota?.accountId)
    : null;
  const pinnedWindow = preferences.pinnedQuota
    ? pinnedAccount?.windows.find((window) => window.id === preferences.pinnedQuota?.windowId)
    : null;
  const pinnedReset = pinnedWindow
    ? menuBarResetCountdown(pinnedWindow.resetsAt, now)
    : null;
  const pinnedDisplay = pinnedWindow
    ? preferences.pinnedQuota?.display === "reset"
      ? pinnedReset
        ? `${menuBarQuotaLabel(pinnedWindow)}:${pinnedReset}`
        : "Unavailable"
      : `${menuBarQuotaLabel(pinnedWindow)}:${displayPercent(pinnedWindow.percentUsed)}%`
    : preferences.pinnedQuota
      ? "Unavailable"
      : null;

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
              codexFlow.reset();
              claudeFlow.reset();
              setConnectOpen(true);
            }}
          >
            <span aria-hidden="true">+</span> Add account
          </button>
        </div>
        <p className="settings__caption">
          Existing sign-ins are detected automatically. Accounts you add here use the
          provider&apos;s official browser sign-in.
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
        <h2 className="settings__title">Usage updates</h2>
        <div className="settings__refreshChoice">
          <label htmlFor="refresh-interval">
            <span className="settings__trayLabel">Auto-refresh</span>
            <span className="settings__trayDescription">
              Re-check providers and update the pinned menu-bar percentage.
            </span>
          </label>
          <select
            id="refresh-interval"
            aria-label="Auto-refresh interval"
            value={preferences.refreshIntervalSeconds}
            onChange={(event) => {
              const selected = REFRESH_INTERVAL_OPTIONS.find(
                (option) => option.value === Number(event.currentTarget.value),
              );
              if (!selected) return;
              onChange({ ...preferences, refreshIntervalSeconds: selected.value });
              onRefreshAccounts();
            }}
          >
            {REFRESH_INTERVAL_OPTIONS.map((option) => (
              <option value={option.value} key={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </div>
      </section>

      <section className="settings__section">
        <h2 className="settings__title">Startup</h2>
        <Switch
          label="Launch at login"
          description="Start quietly in the menu bar when you sign in to this Mac."
          checked={launchAtLogin?.enabled ?? false}
          disabled={!launchAtLogin?.available || launchAtLogin.busy}
          onChange={(enabled) => void launchAtLogin?.setEnabled(enabled)}
        />
        {launchAtLogin?.error ? (
          <p className="settings__warning" role="status">
            {launchAtLogin.error}
          </p>
        ) : null}
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
        <div className="themechooser" role="radiogroup" aria-label="Color theme">
          {THEME_OPTIONS.map((theme) => (
            <button
              type="button"
              className="themechooser__option"
              data-theme-preview={theme.id}
              role="radio"
              aria-checked={preferences.appearanceTheme === theme.id}
              key={theme.id}
              onClick={() => onChange({ ...preferences, appearanceTheme: theme.id })}
            >
              <span className="themechooser__preview" aria-hidden="true">
                <i />
                <i />
                <i />
              </span>
              <span className="themechooser__copy">
                <strong>{theme.name}</strong>
                <small>{theme.description}</small>
              </span>
              <span className="themechooser__check" aria-hidden="true">✓</span>
            </button>
          ))}
        </div>
        <div className="backgroundchooser">
          <div className="backgroundchooser__head">
            <span className="settings__trayLabel">Background</span>
            <span className="settings__trayDescription">Change the atmosphere, keep the data.</span>
          </div>
          <div className="backgroundchooser__options" role="radiogroup" aria-label="Background style">
            {BACKGROUND_OPTIONS.map((background) => (
              <button
                type="button"
                role="radio"
                aria-checked={preferences.backgroundStyle === background.id}
                key={background.id}
                onClick={() => onChange({ ...preferences, backgroundStyle: background.id })}
              >
                <span data-background-preview={background.id} aria-hidden="true" />
                {background.label}
              </button>
            ))}
          </div>
        </div>
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
                ? `${PROVIDER_META[pinnedAccount.provider].name} · ${pinnedAccount.label} · ${pinnedWindow.label} · ${preferences.pinnedQuota?.display === "reset" ? "reset timer" : "live usage"}`
                : preferences.pinnedQuota
                  ? "The pinned quota is temporarily unavailable; the logo is showing."
                  : "Click a quota for live usage, or its reset time for a countdown."}
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
                    : "Choose a provider. Claude and OpenAI can add isolated accounts; other providers check their existing sign-ins."}
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
                {activeFlow &&
                (connectProvider === "openai" || connectProvider === "claude") &&
                activeFlow.login.status !== "idle" ? (
                  <p
                    className="connectsheet__note"
                    role={activeFlow.login.status === "error" ? "alert" : "status"}
                  >
                    {activeFlow.login.status === "starting"
                      ? LOGIN_FLOW_COPY[connectProvider].starting
                      : activeFlow.login.status === "waiting"
                        ? LOGIN_FLOW_COPY[connectProvider].waiting
                        : activeFlow.login.status === "success"
                          ? LOGIN_FLOW_COPY[connectProvider].success
                          : activeFlow.login.message}
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
                      if (provider === "openai") codexFlow.reset();
                      if (provider === "claude") claudeFlow.reset();
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
              {activeFlow && (connectProvider === "openai" || connectProvider === "claude") ? (
                <button
                  type="button"
                  className="btn btn--primary"
                  disabled={
                    activeFlow.login.status === "starting" ||
                    activeFlow.login.status === "waiting"
                  }
                  onClick={() => {
                    if (activeFlow.login.status === "success") {
                      setConnectOpen(false);
                    } else {
                      void activeFlow.begin();
                    }
                  }}
                >
                  {activeFlow.login.status === "starting"
                    ? "Starting…"
                    : activeFlow.login.status === "waiting"
                      ? "Waiting for browser…"
                      : activeFlow.login.status === "success"
                        ? "Done"
                        : activeFlow.login.status === "error"
                          ? "Try again"
                          : LOGIN_FLOW_COPY[connectProvider].add}
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
