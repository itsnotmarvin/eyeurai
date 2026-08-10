import type { ReactNode } from "react";

import { EmptyBoxIcon } from "./Icons";

export interface EmptyStateProps {
  title: string;
  body: ReactNode;
  actionLabel?: string;
  onAction?: () => void;
  tone?: "neutral" | "error";
}

export function EmptyState({ title, body, actionLabel, onAction, tone = "neutral" }: EmptyStateProps) {
  return (
    <div className="empty" data-tone={tone} role={tone === "error" ? "alert" : "status"}>
      <span className="empty__icon" aria-hidden="true">
        <EmptyBoxIcon size={26} />
      </span>
      <h2 className="empty__title">{title}</h2>
      <p className="empty__body">{body}</p>
      {actionLabel && onAction ? (
        <button type="button" className="btn btn--primary btn--small" onClick={onAction}>
          {actionLabel}
        </button>
      ) : null}
    </div>
  );
}
