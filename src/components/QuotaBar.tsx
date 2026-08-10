import { memo } from "react";

import type { QuotaWindow } from "../types/quota";
import {
  displayPercent,
  formatResetCountdown,
  formatUsage,
  severityFor,
  usageAriaText,
} from "../lib/format";
import { ClockIcon, PinIcon } from "./Icons";

export interface QuotaBarProps {
  window: QuotaWindow;
  now: number;
  warnThreshold: number;
  criticalThreshold: number;
  /** Account context so the accessible name is unambiguous. */
  accountName: string;
  isPinned?: boolean;
  onTogglePin?: () => void;
}

function QuotaBarImpl({
  window,
  now,
  warnThreshold,
  criticalThreshold,
  accountName,
  isPinned = false,
  onTogglePin,
}: QuotaBarProps) {
  const percent = displayPercent(window.percentUsed);
  const severity = severityFor(window.percentUsed, warnThreshold, criticalThreshold);
  const countdown = formatResetCountdown(window.resetsAt, now);
  const quotaName = window.note ? `${window.label} (${window.note})` : window.label;
  const pinLabel = isPinned
    ? `Unpin ${quotaName} quota for ${accountName} from menu bar`
    : `Pin ${quotaName} quota for ${accountName} to menu bar`;

  return (
    <li
      className="quota"
      data-severity={severity}
      data-pinned={isPinned ? "true" : undefined}
    >
      <div className="quota__head">
        <span className="quota__label">
          {window.label}
          {window.note ? <span className="quota__note">{window.note}</span> : null}
          <span className="quota__pinState" aria-hidden="true">
            <PinIcon size={10} />
            {isPinned ? "Pinned" : "Pin"}
          </span>
        </span>
        <span className="quota__percent">{percent}%</span>
      </div>

      <div
        className="quota__track"
        role="progressbar"
        aria-label={`${window.label} usage for ${accountName}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={usageAriaText(window, now)}
      >
        <span
          className="quota__threshold"
          style={{ left: `${warnThreshold}%` }}
          aria-hidden="true"
        />
        <span
          className="quota__threshold quota__threshold--critical"
          style={{ left: `${criticalThreshold}%` }}
          aria-hidden="true"
        />
        <span className="quota__fill" style={{ width: `${percent}%` }} />
      </div>

      <div className="quota__meta">
        <span className="quota__usage">{formatUsage(window)}</span>
        {countdown ? (
          <span className="quota__reset">
            <ClockIcon size={11} />
            {countdown}
          </span>
        ) : (
          <span className="quota__reset quota__reset--none">no reset</span>
        )}
      </div>
      {onTogglePin ? (
        <button
          type="button"
          className="quota__pinTarget"
          aria-label={pinLabel}
          aria-pressed={isPinned}
          title={pinLabel}
          onClick={onTogglePin}
        />
      ) : null}
    </li>
  );
}

export const QuotaBar = memo(QuotaBarImpl);
