import { PROVIDER_META, type Account, type Severity } from "../types/quota";
import { displayPercent, formatRelativeTime, formatResetCountdown, severityFor } from "../lib/format";

export interface StatusBarProps {
  accounts: Account[];
  generatedAt: string | null;
  now: number;
  warnThreshold: number;
  criticalThreshold: number;
  mode: "live" | "demo";
}

interface Headline {
  severity: Severity;
  text: string;
  detail: string | null;
}

function buildHeadline(
  accounts: Account[],
  now: number,
  warnThreshold: number,
  criticalThreshold: number,
): Headline {
  const errors = accounts.filter((account) => account.status === "error").length;
  let peak: { percent: number; text: string; resetsAt: string | null } | null = null;

  for (const account of accounts) {
    // The headline is a live summary. Stale/error rows still render in their
    // cards with context, but must never look like current quota pressure.
    if (account.status !== "fresh") continue;
    for (const window of account.windows) {
      if (peak === null || window.percentUsed > peak.percent) {
        peak = {
          percent: window.percentUsed,
          text: `${PROVIDER_META[account.provider].name} · ${window.label}`,
          resetsAt: window.resetsAt,
        };
      }
    }
  }

  if (errors > 0 && (peak === null || peak.percent < criticalThreshold)) {
    return {
      severity: "critical",
      text: `${errors} account${errors === 1 ? "" : "s"} need attention`,
      detail: null,
    };
  }

  if (peak === null) {
    return { severity: "normal", text: "No quota data yet", detail: null };
  }

  const severity = severityFor(peak.percent, warnThreshold, criticalThreshold);
  return {
    severity,
    text: `${peak.text} · ${displayPercent(peak.percent)}%`,
    detail: formatResetCountdown(peak.resetsAt, now),
  };
}

export function StatusBar({
  accounts,
  generatedAt,
  now,
  warnThreshold,
  criticalThreshold,
  mode,
}: StatusBarProps) {
  const headline = buildHeadline(accounts, now, warnThreshold, criticalThreshold);

  return (
    <div className="statusbar" data-severity={headline.severity}>
      <p className="statusbar__text">
        <span className="statusbar__headline">{headline.text}</span>
        {headline.detail ? <span className="statusbar__detail">{headline.detail}</span> : null}
      </p>
      <span className="statusbar__meta">
        {mode === "demo" ? <span className="statusbar__badge">demo</span> : null}
        {formatRelativeTime(generatedAt, now).replace("updated ", "")}
      </span>
    </div>
  );
}
