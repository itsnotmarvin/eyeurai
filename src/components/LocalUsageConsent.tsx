import { useModalDialog } from "../hooks/useModalDialog";

export interface LocalUsageConsentProps {
  onAllow: () => void;
  onClose: () => void;
}

export function LocalUsageConsent({ onAllow, onClose }: LocalUsageConsentProps) {
  const dialogRef = useModalDialog(true, onClose);
  return (
    <div
      className="connectsheet"
      role="dialog"
      aria-modal="true"
      aria-label="Local usage access"
      ref={dialogRef}
    >
      <div className="connectsheet__card usageconsent">
        <div className="connectsheet__head">
          <div>
            <h2 className="connectsheet__title">Allow local usage totals?</h2>
            <p className="connectsheet__subtitle">Optional, read-only access on this computer.</p>
          </div>
          <button
            type="button"
            className="connectsheet__close"
            aria-label="Close local usage access"
            onClick={onClose}
          >
            ×
          </button>
        </div>
        <div className="usageconsent__body">
          <p>
            EyeUrAI will scan usage counters inside Claude Code and Codex session files in:
          </p>
          <code>~/.claude/projects</code>
          <code>~/.codex/sessions</code>
          <p>
            Those files can also contain conversation data. EyeUrAI extracts aggregate token
            numbers locally, never returns prompt text to the interface, and never uploads it.
          </p>
        </div>
        <div className="connectsheet__actions">
          <button type="button" className="btn btn--ghost" onClick={onClose}>
            Not now
          </button>
          <button type="button" className="btn btn--primary" onClick={onAllow}>
            Allow local scan
          </button>
        </div>
      </div>
    </div>
  );
}
