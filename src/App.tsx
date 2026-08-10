import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import {
  PROVIDER_META,
  PROVIDER_ORDER,
  type Account,
  type DisconnectedAccount,
  type LocalUsageSnapshot,
  type ProviderId,
} from "./types/quota";
import { useNow } from "./hooks/useNow";
import { usePreferences } from "./hooks/usePreferences";
import { useSnapshot } from "./hooks/useSnapshot";
import { useAppUpdate } from "./hooks/useAppUpdate";
import { fetchLocalUsage, hideWindow, setTrayDisplay } from "./lib/ipc";
import { createDemoLocalUsage } from "./lib/demo";
import { displayPercent, menuBarQuotaLabel } from "./lib/format";
import { computeAlerts, deliverAlerts, type AlertState } from "./lib/notify";
import { EmptyState } from "./components/EmptyState";
import { Header } from "./components/Header";
import { Onboarding } from "./components/Onboarding";
import { ProviderSection } from "./components/ProviderSection";
import { SettingsView } from "./components/SettingsView";
import { LocalUsageConsent } from "./components/LocalUsageConsent";
import { LocalUsagePanel } from "./components/LocalUsagePanel";
import { SkeletonList } from "./components/Skeleton";
import { StatusBar } from "./components/StatusBar";
import { UpdateDialog } from "./components/UpdateDialog";

type View = "dashboard" | "settings";

function groupByProvider(accounts: Account[]): Array<[ProviderId, Account[]]> {
  const groups = new Map<ProviderId, Account[]>();
  for (const account of accounts) {
    const bucket = groups.get(account.provider);
    if (bucket) bucket.push(account);
    else groups.set(account.provider, [account]);
  }
  return PROVIDER_ORDER.filter((provider) => groups.has(provider)).map((provider) => [
    provider,
    groups.get(provider) ?? [],
  ]);
}

/** True when focus is inside a control that owns its own key handling. */
function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select" || target.isContentEditable;
}

export function App() {
  const { preferences, replace, update } = usePreferences();
  const appUpdate = useAppUpdate();
  const excludedAccountIds = useMemo(
    () => preferences.disconnectedAccounts.map((account) => account.id),
    [preferences.disconnectedAccounts],
  );
  const { snapshot, phase, mode, refreshing, error, refresh } =
    useSnapshot(excludedAccountIds);
  const now = useNow();
  const [view, setView] = useState<View>("dashboard");
  const [localUsage, setLocalUsage] = useState<LocalUsageSnapshot | null>(null);
  const [localUsageDays, setLocalUsageDays] = useState<7 | 30 | 90>(7);
  const [localUsageLoading, setLocalUsageLoading] = useState(false);
  const [localUsageError, setLocalUsageError] = useState<string | null>(null);
  const [usageConsentOpen, setUsageConsentOpen] = useState(false);
  const [updateDialogOpen, setUpdateDialogOpen] = useState(false);
  const alertState = useRef<AlertState>({});
  const scrollRef = useRef<HTMLElement | null>(null);

  const monitoredSnapshot = useMemo(() => {
    if (!snapshot) return null;
    const excluded = new Set(excludedAccountIds);
    const disconnectedProviders = new Set(
      preferences.disconnectedAccounts.map((account) => account.provider),
    );
    return {
      ...snapshot,
      accounts: snapshot.accounts.filter(
        (account) =>
          !excluded.has(account.id) &&
          !(/-status-\d+$/.test(account.id) && disconnectedProviders.has(account.provider)),
      ),
    };
  }, [excludedAccountIds, preferences.disconnectedAccounts, snapshot]);

  const visibleAccounts = useMemo(() => {
    if (!monitoredSnapshot) return [];
    return monitoredSnapshot.accounts.filter((account) =>
      preferences.enabledProviders.includes(account.provider),
    );
  }, [monitoredSnapshot, preferences.enabledProviders]);

  const groups = useMemo(() => groupByProvider(visibleAccounts), [visibleAccounts]);
  const pinnedQuotaDetails = useMemo(() => {
    const pinned = preferences.pinnedQuota;
    if (!pinned || !monitoredSnapshot) return null;
    const account = monitoredSnapshot.accounts.find((candidate) => candidate.id === pinned.accountId);
    const window = account?.windows.find((candidate) => candidate.id === pinned.windowId);
    return account && window
      ? { account, window, windowLabel: menuBarQuotaLabel(window) }
      : null;
  }, [monitoredSnapshot, preferences.pinnedQuota]);
  const pinnedTrayLabel = pinnedQuotaDetails?.windowLabel ?? null;
  const pinnedTrayPercent = pinnedQuotaDetails
    ? displayPercent(pinnedQuotaDetails.window.percentUsed)
    : null;

  useEffect(() => {
    void setTrayDisplay(pinnedTrayLabel, pinnedTrayPercent);
  }, [pinnedTrayLabel, pinnedTrayPercent]);

  const togglePinnedQuota = useCallback(
    (accountId: string, windowId: string) => {
      const current = preferences.pinnedQuota;
      update({
        pinnedQuota:
          current?.accountId === accountId && current.windowId === windowId
            ? null
            : { accountId, windowId },
      });
    },
    [preferences.pinnedQuota, update],
  );

  // Threshold notifications. Alerts fire only on escalation, never per poll.
  useEffect(() => {
    if (!monitoredSnapshot) return;
    const { alerts, state } = computeAlerts(monitoredSnapshot, preferences, alertState.current);
    alertState.current = state;
    if (preferences.notificationsEnabled && alerts.length > 0) {
      void deliverAlerts(alerts);
    }
  }, [monitoredSnapshot, preferences]);

  const disconnectAccount = useCallback(
    (account: Account) => {
      if (preferences.disconnectedAccounts.some((entry) => entry.id === account.id)) return;
      replace({
        ...preferences,
        pinnedQuota:
          preferences.pinnedQuota?.accountId === account.id
            ? null
            : preferences.pinnedQuota,
        disconnectedAccounts: [
          ...preferences.disconnectedAccounts,
          { id: account.id, provider: account.provider, label: account.label },
        ],
      });
    },
    [preferences, replace],
  );

  const reconnectAccount = useCallback(
    (account: DisconnectedAccount) => {
      replace({
        ...preferences,
        enabledProviders: preferences.enabledProviders.includes(account.provider)
          ? preferences.enabledProviders
          : PROVIDER_ORDER.filter((provider) =>
              [...preferences.enabledProviders, account.provider].includes(provider),
            ),
        disconnectedAccounts: preferences.disconnectedAccounts.filter(
          (entry) => entry.id !== account.id,
        ),
      });
    },
    [preferences, replace],
  );

  const scanLocalUsage = useCallback(async () => {
    if (!preferences.localUsageEnabled) {
      setLocalUsage(null);
      setLocalUsageLoading(false);
      setLocalUsageError(null);
      return;
    }
    setLocalUsageLoading(true);
    setLocalUsageError(null);
    try {
      const next =
        mode === "demo"
          ? createDemoLocalUsage(Date.now(), localUsageDays)
          : await fetchLocalUsage(localUsageDays);
      setLocalUsage(next);
    } catch (cause) {
      setLocalUsageError(
        cause instanceof Error ? cause.message : "Could not read local token counters.",
      );
    } finally {
      setLocalUsageLoading(false);
    }
  }, [localUsageDays, mode, preferences.localUsageEnabled]);

  useEffect(() => {
    void scanLocalUsage();
  }, [scanLocalUsage]);

  const refreshAll = useCallback(() => {
    refresh();
    if (preferences.localUsageEnabled) void scanLocalUsage();
  }, [preferences.localUsageEnabled, refresh, scanLocalUsage]);

  const openSettings = useCallback(() => setView("settings"), []);
  const closeSettings = useCallback(() => setView("dashboard"), []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === "Escape") {
        event.preventDefault();
        if (view === "settings") closeSettings();
        else void hideWindow();
        return;
      }
      if (isTypingTarget(event.target)) return;
      if (event.key === "r" || event.key === "R") {
        event.preventDefault();
        refreshAll();
        return;
      }
      if (event.key === ",") {
        event.preventDefault();
        setView((current) => (current === "settings" ? "dashboard" : "settings"));
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeSettings, refreshAll, view]);

  // Keep the scroll position sane when switching views.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [view]);

  if (!preferences.onboardingCompleted) {
    return (
      <div className="app" data-compact={preferences.compact ? "true" : undefined}>
        <div className="app__frame">
          <Onboarding initial={preferences} onComplete={replace} />
        </div>
      </div>
    );
  }

  const hiddenProviders = PROVIDER_ORDER.filter(
    (provider) => !preferences.enabledProviders.includes(provider),
  );

  let body: ReactNode;
  if (view === "settings") {
    body = (
      <SettingsView
        preferences={preferences}
        onChange={replace}
        mode={mode}
        accounts={monitoredSnapshot?.accounts ?? []}
        onDisconnectAccount={disconnectAccount}
        onReconnectAccount={reconnectAccount}
        onRefreshAccounts={refreshAll}
        onRequestLocalUsage={() => setUsageConsentOpen(true)}
        onRerunSetup={() => {
          update({ onboardingCompleted: false });
          setView("dashboard");
        }}
      />
    );
  } else if (phase === "loading" && !snapshot) {
    body = <SkeletonList count={3} />;
  } else if (phase === "error" && !snapshot) {
    body = (
      <EmptyState
        tone="error"
        title="Could not read quotas"
        body={error ?? "The local agent did not respond."}
        actionLabel="Try again"
        onAction={refreshAll}
      />
    );
  } else if (preferences.enabledProviders.length === 0) {
    body = (
      <EmptyState
        title="Every provider is hidden"
        body="Turn at least one provider back on to see its limits."
        actionLabel="Manage providers"
        onAction={openSettings}
      />
    );
  } else if (visibleAccounts.length === 0) {
    body = (
      <EmptyState
        title="No connected accounts"
        body={`Sign in to ${preferences.enabledProviders
          .map((provider) => PROVIDER_META[provider].name)
          .join(", ")} with its CLI or add an API key, then refresh.`}
        actionLabel="Refresh"
        onAction={refreshAll}
      />
    );
  } else {
    body = (
      <>
        {groups.map(([provider, accounts]) => (
          <ProviderSection
            key={provider}
            provider={provider}
            accounts={accounts}
            now={now}
            warnThreshold={preferences.warnThreshold}
            criticalThreshold={preferences.criticalThreshold}
            pinnedQuota={preferences.pinnedQuota}
            onToggleQuotaPin={togglePinnedQuota}
            onRetry={refreshAll}
          />
        ))}
        {hiddenProviders.length > 0 ? (
          <p className="hiddennote">
            {hiddenProviders.length} provider{hiddenProviders.length === 1 ? "" : "s"} hidden ·{" "}
            <button type="button" className="linkbtn" onClick={openSettings}>
              manage
            </button>
          </p>
        ) : null}
        <LocalUsagePanel
          enabled={preferences.localUsageEnabled}
          usage={localUsage}
          loading={localUsageLoading}
          error={localUsageError}
          rangeDays={localUsageDays}
          onRangeChange={setLocalUsageDays}
          onReviewAccess={() => setUsageConsentOpen(true)}
        />
      </>
    );
  }

  return (
    <div className="app" data-compact={preferences.compact ? "true" : undefined}>
      <div className="app__frame">
        <Header
          variant={view === "settings" ? "settings" : "dashboard"}
          title="Settings"
          refreshing={refreshing}
          onRefresh={refreshAll}
          onOpenSettings={openSettings}
          onBack={closeSettings}
          updateVersion={appUpdate.info?.version ?? null}
          onOpenUpdate={() => setUpdateDialogOpen(true)}
        />

        {view === "dashboard" ? (
          <StatusBar
            accounts={visibleAccounts}
            generatedAt={monitoredSnapshot?.generatedAt ?? null}
            now={now}
            warnThreshold={preferences.warnThreshold}
            criticalThreshold={preferences.criticalThreshold}
            mode={mode}
          />
        ) : null}

        <main
          className="app__scroll"
          ref={scrollRef}
          tabIndex={0}
          aria-label={view === "settings" ? "Settings" : "Connected accounts"}
        >
          {error && snapshot && view === "dashboard" ? (
            <p className="app__banner" role="status">
              {error} · showing last known values
            </p>
          ) : null}
          {body}
        </main>
        {usageConsentOpen ? (
          <LocalUsageConsent
            onClose={() => setUsageConsentOpen(false)}
            onAllow={() => {
              update({ localUsageEnabled: true });
              setUsageConsentOpen(false);
            }}
          />
        ) : null}
        {updateDialogOpen && appUpdate.info ? (
          <UpdateDialog
            info={appUpdate.info}
            progress={appUpdate.progress}
            error={appUpdate.error}
            onInstall={() => void appUpdate.install()}
            onClose={() => setUpdateDialogOpen(false)}
          />
        ) : null}
      </div>
    </div>
  );
}
