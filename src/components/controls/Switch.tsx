import { useId } from "react";

export interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: string;
  disabled?: boolean;
}

/** Accessible switch: `role="switch"`, toggled with Space/Enter. */
export function Switch({ checked, onChange, label, description, disabled }: SwitchProps) {
  const descriptionId = useId();

  return (
    <div className="switchrow">
      <div className="switchrow__text">
        <span className="switchrow__label" id={`${descriptionId}-label`}>
          {label}
        </span>
        {description ? (
          <span className="switchrow__description" id={descriptionId}>
            {description}
          </span>
        ) : null}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-labelledby={`${descriptionId}-label`}
        aria-describedby={description ? descriptionId : undefined}
        className="switch"
        disabled={disabled}
        onClick={() => onChange(!checked)}
      >
        <span className="switch__thumb" aria-hidden="true" />
      </button>
    </div>
  );
}
