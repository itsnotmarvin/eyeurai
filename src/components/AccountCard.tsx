import { memo } from "react";

import {
  PROVIDER_META,
  type Account,
  type AuthSource,
  type PinnedQuota,
  type PinnedQuotaDisplay,
} from "../types/quota";
import { formatRelativeTime } from "../lib/format";
import { AlertIcon, RefreshIcon } from "./Icons";
import { QuotaBar } from "./QuotaBar";

const SOURCE_LABEL: Record<AuthSource, string> = {
  oauth: "OAuth",
  "api-key": "API key",
  cli: "CLI",
  unknown: "Local",
};

const STATUS_LABEL: Record<Account["status"], string> = {
  fresh: "Live",
  stale: "Stale",
  error: "Error",
  pending: "Reading…",
  unconfigured: "Not connected",
  unsupported: "Unavailable",
};

function isRetainedCliAccount(account: Account): boolean {
  return (
    (account.provider === "claude" || account.provider === "openai") &&
    account.isCliActive === false &&
    account.status !== "fresh"
  );
}

function statusLabel(account: Account): string {
  if (!isRetainedCliAccount(account)) return STATUS_LABEL[account.status];
  if (account.status === "fresh") return "Cached";
  if (account.status === "stale") return "Last known";
  return STATUS_LABEL[account.status];
}

export interface AccountCardProps {
  account: Account;
  now: number;
  warnThreshold: number;
  criticalThreshold: number;
  pinnedQuota?: PinnedQuota | null;
  onToggleQuotaPin?: (
    accountId: string,
    windowId: string,
    display: PinnedQuotaDisplay,
  ) => void;
  onRetry?: () => void;
  onRemediate?: (account: Account) => void;
}

function AccountCardImpl({
  account,
  now,
  warnThreshold,
  criticalThreshold,
  pinnedQuota,
  onToggleQuotaPin,
  onRetry,
  onRemediate,
}: AccountCardProps) {
  const meta = PROVIDER_META[account.provider];
  const accountName = `${meta.name} · ${account.label}`;
  const retainedCliAccount = isRetainedCliAccount(account);

  return (
    <article
      className="account"
      data-provider={account.provider}
      data-status={account.status}
      aria-label={accountName}
    >
      <header className="account__head">
        <div className="account__identity">
          <span className="account__name" title={account.label}>
            {account.label}
          </span>
          <span className="account__sub">
            {account.plan ? <span className="account__plan">{account.plan}</span> : null}
            <span className="account__source">{SOURCE_LABEL[account.source]}</span>
          </span>
        </div>
        <span
          className="account__status"
          data-status={account.status}
          data-retained={retainedCliAccount ? "true" : undefined}
        >
          <span className="account__statusMark" aria-hidden="true" />
          {statusLabel(account)}
        </span>
      </header>

      {account.windows.length > 0 ? (
        <ul className="account__quotas">
          {account.windows.map((window) => (
            <QuotaBar
              key={window.id}
              window={window}
              now={now}
              warnThreshold={warnThreshold}
              criticalThreshold={criticalThreshold}
              accountName={accountName}
              isPinned={
                pinnedQuota?.accountId === account.id && pinnedQuota.windowId === window.id
              }
              pinnedDisplay={
                pinnedQuota?.accountId === account.id && pinnedQuota.windowId === window.id
                  ? (pinnedQuota.display ?? "usage")
                  : undefined
              }
              onTogglePin={
                onToggleQuotaPin
                  ? (display) => onToggleQuotaPin(account.id, window.id, display)
                  : undefined
              }
            />
          ))}
        </ul>
      ) : (
        <p className="account__nodata">No quota windows reported yet.</p>
      )}

      {account.message ? (
        <p className="account__message" role={account.status === "error" ? "alert" : undefined}>
          <AlertIcon size={13} />
          <span>{account.message}</span>
          {account.remediation && onRemediate ? (
            <button type="button" className="account__retry" onClick={() => onRemediate(account)}>
              {account.remediation.choices[0]?.kind === "retry" ? <RefreshIcon size={12} /> : null}
              {account.remediation.choices[0]?.kind === "retry"
                ? "Try again"
                : account.remediation.choices[0]?.kind === "open-settings"
                  ? account.provider === "openrouter" ? "Connect" : "Manage"
                  : "Reconnect"}
            </button>
          ) : account.status === "error" && onRetry ? (
            <button type="button" className="account__retry" onClick={onRetry}>
              <RefreshIcon size={12} />
              Retry
            </button>
          ) : null}
        </p>
      ) : null}

      <footer className="account__foot">{formatRelativeTime(account.updatedAt, now)}</footer>
    </article>
  );
}

export const AccountCard = memo(AccountCardImpl);
