import type { AppUpdateInfo, AppUpdateProgress } from "../lib/appUpdates";
import { useModalDialog } from "../hooks/useModalDialog";

export interface UpdateDialogProps {
  info: AppUpdateInfo;
  progress: AppUpdateProgress | null;
  error: string | null;
  onInstall: () => void;
  onClose: () => void;
}

function actionLabel(progress: AppUpdateProgress | null, error: string | null): string {
  if (progress?.stage === "installing") return "Installing…";
  if (progress?.stage === "downloading") {
    return progress.percent === null ? "Downloading…" : `Downloading ${progress.percent}%`;
  }
  return error ? "Try again" : "Update & restart";
}

export function UpdateDialog({
  info,
  progress,
  error,
  onInstall,
  onClose,
}: UpdateDialogProps) {
  const busy = progress !== null;
  const progressValue = progress?.percent ?? undefined;
  const dialogRef = useModalDialog(true, busy ? undefined : onClose);

  return (
    <div
      className="connectsheet"
      role="dialog"
      aria-modal="true"
      aria-label="EyeUrAI update"
      ref={dialogRef}
    >
      <div className="connectsheet__card updatedialog">
        <div className="connectsheet__head">
          <div>
            <h2 className="connectsheet__title">EyeUrAI {info.version} is available</h2>
            <p className="connectsheet__subtitle">
              You’re currently using {info.currentVersion}.
            </p>
          </div>
          <button
            type="button"
            className="connectsheet__close"
            aria-label="Close update details"
            disabled={busy}
            onClick={onClose}
          >
            ×
          </button>
        </div>

        <div className="updatedialog__body">
          <p>EyeUrAI will download the signed update, close, install it, and reopen automatically.</p>
          {info.notes ? (
            <div className="updatedialog__notes" aria-label="Release notes">
              {info.notes}
            </div>
          ) : null}
          {progress ? (
            <div className="updatedialog__progress">
              <div
                className="updatedialog__track"
                role="progressbar"
                aria-label={progress.stage === "installing" ? "Installing update" : "Downloading update"}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={progressValue}
              >
                <span
                  style={{ width: progressValue === undefined ? "35%" : `${progressValue}%` }}
                  data-indeterminate={progressValue === undefined ? "true" : undefined}
                />
              </div>
            </div>
          ) : null}
          {error ? <p className="updatedialog__error" role="alert">{error}</p> : null}
        </div>

        <div className="connectsheet__actions">
          <button type="button" className="btn btn--ghost" disabled={busy} onClick={onClose}>
            Later
          </button>
          <button type="button" className="btn btn--primary" disabled={busy} onClick={onInstall}>
            {actionLabel(progress, error)}
          </button>
        </div>
      </div>
    </div>
  );
}
