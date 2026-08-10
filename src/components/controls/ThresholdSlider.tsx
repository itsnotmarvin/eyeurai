import { useId, type CSSProperties } from "react";

import { THRESHOLD_MAX, THRESHOLD_MIN } from "../../lib/preferences";

export interface ThresholdSliderProps {
  label: string;
  description?: string;
  value: number;
  onChange: (value: number) => void;
  tone: "warn" | "critical";
  disabled?: boolean;
  min?: number;
  max?: number;
}

/**
 * Native range input so arrow keys, Home/End and PageUp/PageDown all work for
 * free; only the visuals are restyled.
 */
export function ThresholdSlider({
  label,
  description,
  value,
  onChange,
  tone,
  disabled,
  min = THRESHOLD_MIN,
  max = THRESHOLD_MAX,
}: ThresholdSliderProps) {
  const id = useId();
  const percent = ((value - min) / Math.max(1, max - min)) * 100;

  return (
    <div className="slider" data-tone={tone} data-disabled={disabled ? "true" : undefined}>
      <div className="slider__head">
        <label className="slider__label" htmlFor={id}>
          {label}
        </label>
        <output className="slider__value" htmlFor={id}>
          {value}%
        </output>
      </div>
      <input
        id={id}
        type="range"
        className="slider__input"
        min={min}
        max={max}
        step={1}
        value={value}
        disabled={disabled}
        aria-describedby={description ? `${id}-hint` : undefined}
        style={{ "--slider-progress": `${percent}%` } as CSSProperties}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      {description ? (
        <p className="slider__hint" id={`${id}-hint`}>
          {description}
        </p>
      ) : null}
    </div>
  );
}
