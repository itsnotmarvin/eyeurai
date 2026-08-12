import { ChevronLeftIcon, EyeMark, RefreshIcon, SettingsIcon } from "./Icons";

export interface HeaderProps {
  /** Dashboard shows brand + actions; settings shows a back button. */
  variant: "dashboard" | "settings";
  title?: string;
  refreshing?: boolean;
  onRefresh?: () => void;
  onOpenSettings?: () => void;
  onBack?: () => void;
  updateVersion?: string | null;
  onOpenUpdate?: () => void;
}

export function Header({
  variant,
  title,
  refreshing = false,
  onRefresh,
  onOpenSettings,
  onBack,
  updateVersion = null,
  onOpenUpdate,
}: HeaderProps) {
  return (
    <header className="header" data-variant={variant} data-tauri-drag-region>
      {variant === "dashboard" ? (
        <div className="header__brand" data-tauri-drag-region>
          <span className="header__logo" aria-hidden="true" data-tauri-drag-region>
            <EyeMark size={18} />
          </span>
          <span className="header__wordmark" data-tauri-drag-region>
            Eye<span className="header__wordmark-accent">Ur</span>AI
          </span>
        </div>
      ) : (
        <button type="button" className="header__back" onClick={onBack} aria-label="Back to quotas">
          <ChevronLeftIcon size={16} />
          <span>{title ?? "Back"}</span>
        </button>
      )}

      {variant === "dashboard" ? (
        <div className="header__actions">
          {updateVersion ? (
            <button
              type="button"
              className="header__update"
              onClick={onOpenUpdate}
              aria-label={`Update available: EyeUrAI ${updateVersion}`}
              title={`Install EyeUrAI ${updateVersion}`}
            >
              <span aria-hidden="true" />
              Update available
            </button>
          ) : null}
          <button
            type="button"
            className="iconbtn"
            onClick={onRefresh}
            data-spinning={refreshing ? "true" : undefined}
            aria-label="Refresh quotas"
            title="Refresh quotas (R)"
          >
            <RefreshIcon size={15} />
          </button>
          <button
            type="button"
            className="iconbtn"
            onClick={onOpenSettings}
            aria-label="Open settings"
            title="Settings (,)"
          >
            <SettingsIcon size={15} />
          </button>
        </div>
      ) : null}
    </header>
  );
}
