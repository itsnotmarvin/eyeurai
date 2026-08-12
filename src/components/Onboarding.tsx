import { useState } from "react";

import { PROVIDER_META, PROVIDER_ORDER, type Preferences, type ProviderId } from "../types/quota";
import { reconcileThresholds } from "../lib/preferences";
import { ensureNotificationPermission } from "../lib/notify";
import { CheckIcon, EyeMark } from "./Icons";
import { ProviderMark } from "./ProviderMark";
import { Switch } from "./controls/Switch";
import { ThresholdSlider } from "./controls/ThresholdSlider";

export interface OnboardingProps {
  initial: Preferences;
  onComplete: (preferences: Preferences) => void;
  /** Injected in tests to avoid touching the real permission prompt. */
  requestPermission?: () => Promise<boolean>;
}

const STEPS = ["Providers", "Alerts"] as const;

export function Onboarding({ initial, onComplete, requestPermission }: OnboardingProps) {
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<Preferences>(initial);
  const [permissionDenied, setPermissionDenied] = useState(false);

  const askForPermission = requestPermission ?? ensureNotificationPermission;
  const selected = draft.enabledProviders;
  const canContinue = selected.length > 0;

  function toggleProvider(provider: ProviderId): void {
    setDraft((current) => {
      const next = current.enabledProviders.includes(provider)
        ? current.enabledProviders.filter((id) => id !== provider)
        : [...current.enabledProviders, provider];
      return {
        ...current,
        enabledProviders: PROVIDER_ORDER.filter((id) => next.includes(id)),
      };
    });
  }

  async function toggleNotifications(enabled: boolean): Promise<void> {
    if (!enabled) {
      setPermissionDenied(false);
      setDraft((current) => ({ ...current, notificationsEnabled: false }));
      return;
    }
    const granted = await askForPermission();
    setPermissionDenied(!granted);
    setDraft((current) => ({ ...current, notificationsEnabled: granted }));
  }

  function finish(): void {
    onComplete({ ...draft, onboardingCompleted: true });
  }

  return (
    <div className="onboarding">
      <div className="onboarding__hero">
        <span className="onboarding__logo" aria-hidden="true">
          <EyeMark size={23} />
        </span>
        <h1 className="onboarding__title">
          {step === 0 ? "Which limits should we watch?" : "Tell me before I run out"}
        </h1>
        <p className="onboarding__subtitle">
          {step === 0
            ? "EyeUrAI reads quotas locally. Nothing goes through our servers."
            : "Optional desktop alerts when a quota crosses your thresholds."}
        </p>
      </div>

      <div className="onboarding__body">
        {step === 0 ? (
          <div className="pickers" role="group" aria-label="Providers to monitor">
            {PROVIDER_ORDER.map((provider) => {
              const meta = PROVIDER_META[provider];
              const checked = selected.includes(provider);
              return (
                <button
                  key={provider}
                  type="button"
                  role="checkbox"
                  aria-checked={checked}
                  className="picker"
                  data-provider={provider}
                  data-checked={checked ? "true" : undefined}
                  onClick={() => toggleProvider(provider)}
                >
                  <span className="picker__mark">
                    <ProviderMark provider={provider} size={19} />
                  </span>
                  <span className="picker__text">
                    <span className="picker__name">{meta.name}</span>
                    <span className="picker__blurb">{meta.blurb}</span>
                  </span>
                  <span className="picker__check" aria-hidden="true">
                    {checked ? <CheckIcon size={13} /> : null}
                  </span>
                </button>
              );
            })}
            {!canContinue ? (
              <p className="onboarding__hint" role="status">
                Pick at least one provider to continue.
              </p>
            ) : null}
          </div>
        ) : (
          <div className="onboarding__alerts">
            <Switch
              label="Desktop notifications"
              description="Alert me when usage crosses a threshold."
              checked={draft.notificationsEnabled}
              onChange={(value) => void toggleNotifications(value)}
            />
            {permissionDenied ? (
              <p className="onboarding__hint" role="status">
                Your system denied notifications. You can enable them later in system settings.
              </p>
            ) : null}

            <ThresholdSlider
              label="Warn at"
              tone="warn"
              value={draft.warnThreshold}
              disabled={!draft.notificationsEnabled}
              onChange={(value) =>
                setDraft((current) =>
                  reconcileThresholds({ ...current, warnThreshold: value }, "warn"),
                )
              }
              description="Amber bars and a gentle heads-up."
            />
            <ThresholdSlider
              label="Critical at"
              tone="critical"
              value={draft.criticalThreshold}
              disabled={!draft.notificationsEnabled}
              onChange={(value) =>
                setDraft((current) =>
                  reconcileThresholds({ ...current, criticalThreshold: value }, "critical"),
                )
              }
              description="Red bars and an urgent alert."
            />
          </div>
        )}
      </div>

      <footer className="onboarding__foot">
        <div className="onboarding__steps" aria-hidden="true">
          {STEPS.map((label, index) => (
            <span key={label} className="onboarding__dot" data-active={index === step || undefined} />
          ))}
        </div>
        <div className="onboarding__buttons">
          {step === 0 ? (
            <button type="button" className="btn btn--ghost" onClick={finish}>
              Skip
            </button>
          ) : (
            <button type="button" className="btn btn--ghost" onClick={() => setStep(0)}>
              Back
            </button>
          )}
          {step === 0 ? (
            <button
              type="button"
              className="btn btn--primary"
              disabled={!canContinue}
              onClick={() => setStep(1)}
            >
              Continue
            </button>
          ) : (
            <button type="button" className="btn btn--primary" onClick={finish}>
              Start monitoring
            </button>
          )}
        </div>
      </footer>
    </div>
  );
}
