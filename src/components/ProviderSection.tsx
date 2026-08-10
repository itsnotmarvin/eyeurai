import {
  PROVIDER_META,
  type Account,
  type PinnedQuota,
  type ProviderId,
} from "../types/quota";
import { displayPercent, severityFor } from "../lib/format";
import { AccountCard } from "./AccountCard";
import { ProviderMark } from "./ProviderMark";

export interface ProviderSectionProps {
  provider: ProviderId;
  accounts: Account[];
  now: number;
  warnThreshold: number;
  criticalThreshold: number;
  pinnedQuota?: PinnedQuota | null;
  onToggleQuotaPin?: (accountId: string, windowId: string) => void;
  onRetry?: () => void;
}

/** Highest usage across every window of every account in the group. */
function peakPercent(accounts: Account[]): number | null {
  let peak: number | null = null;
  for (const account of accounts) {
    for (const window of account.windows) {
      if (peak === null || window.percentUsed > peak) peak = window.percentUsed;
    }
  }
  return peak;
}

export function ProviderSection({
  provider,
  accounts,
  now,
  warnThreshold,
  criticalThreshold,
  pinnedQuota,
  onToggleQuotaPin,
  onRetry,
}: ProviderSectionProps) {
  const meta = PROVIDER_META[provider];
  const peak = peakPercent(accounts);
  const severity =
    peak === null ? "normal" : severityFor(peak, warnThreshold, criticalThreshold);

  return (
    <section className="provider" data-provider={provider} aria-label={meta.name}>
      <header className="provider__head">
        <span className="provider__mark">
          <ProviderMark provider={provider} size={15} />
        </span>
        <h2 className="provider__name">{meta.name}</h2>
        <span className="provider__count">
          {accounts.length} {accounts.length === 1 ? "account" : "accounts"}
        </span>
        {peak !== null ? (
          <span className="provider__peak" data-severity={severity}>
            {displayPercent(peak)}% peak
          </span>
        ) : null}
      </header>

      <div className="provider__accounts">
        {accounts.map((account) => (
          <AccountCard
            key={account.id}
            account={account}
            now={now}
            warnThreshold={warnThreshold}
            criticalThreshold={criticalThreshold}
            pinnedQuota={pinnedQuota}
            onToggleQuotaPin={onToggleQuotaPin}
            onRetry={onRetry}
          />
        ))}
      </div>
    </section>
  );
}
