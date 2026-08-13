import { memo } from "react";

import type { PinnedQuotaDisplay, QuotaWindow } from "../types/quota";
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
  pinnedDisplay?: PinnedQuotaDisplay;
  onTogglePin?: (display: PinnedQuotaDisplay) => void;
}

function QuotaBarImpl({
  window,
  now,
  warnThreshold,
  criticalThreshold,
  accountName,
  isPinned = false,
  pinnedDisplay = "usage",
  onTogglePin,
}: QuotaBarProps) {
  const percent = displayPercent(window.percentUsed);
  const severity = severityFor(window.percentUsed, warnThreshold, criticalThreshold);
  const countdown = formatResetCountdown(window.resetsAt, now);
  const quotaName = window.note ? `${window.label} (${window.note})` : window.label;
  const usagePinned = isPinned && pinnedDisplay === "usage";
  const resetPinned = isPinned && pinnedDisplay === "reset";
  const pinLabel = usagePinned
    ? `Unpin ${quotaName} quota for ${accountName} from menu bar`
    : `Pin ${quotaName} quota for ${accountName} to menu bar`;
  const resetPinLabel = resetPinned
    ? `Unpin reset timer for ${quotaName} quota for ${accountName} from menu bar`
    : `Pin reset timer for ${quotaName} quota for ${accountName} to menu bar`;

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
            {resetPinned ? "Timer pinned" : isPinned ? "Pinned" : "Pin"}
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
        <span className="quota__fill" style={{ width: `${percent}%` }} />
      </div>

      <div className="quota__meta">
        <span className="quota__usage">{formatUsage(window)}</span>
        {countdown && onTogglePin ? (
          <button
            type="button"
            className="quota__reset quota__resetPin"
            data-pinned={resetPinned ? "true" : undefined}
            aria-label={resetPinLabel}
            aria-pressed={resetPinned}
            title={resetPinLabel}
            onClick={() => onTogglePin("reset")}
          >
            <ClockIcon size={11} />
            {countdown}
            <PinIcon className="quota__resetPinIcon" size={9} />
          </button>
        ) : countdown ? (
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
          aria-pressed={usagePinned}
          title={pinLabel}
          onClick={() => onTogglePin("usage")}
        />
      ) : null}
    </li>
  );
}

export const QuotaBar = memo(QuotaBarImpl);
