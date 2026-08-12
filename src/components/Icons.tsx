export interface IconProps {
  size?: number;
  className?: string;
}

function base(size: number, className?: string) {
  return {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.7,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    className,
    "aria-hidden": true,
    focusable: false,
  };
}

/** App logo: the EyeUrAI eye and its coral signal glint. */
export function EyeMark({ size = 18, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={1.6}>
      <path d="M1.9 12S5.5 5.4 12 5.4 22.1 12 22.1 12 18.5 18.6 12 18.6 1.9 12 1.9 12Z" />
      <circle cx="12" cy="12" r="3.1" />
      <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
      <path
        className="eye-mark__spark"
        d="m18.1 3.1.82 2.18 2.18.82-2.18.82-.82 2.18-.82-2.18-2.18-.82 2.18-.82.82-2.18Z"
        fill="currentColor"
        stroke="none"
      />
    </svg>
  );
}

export function RefreshIcon({ size = 15, className }: IconProps) {
  return (
    <svg {...base(size, className)}>
      <path d="M20.4 11.2a8.4 8.4 0 1 0-.7 4.6" />
      <path d="M20.6 5.6v5.6H15" />
    </svg>
  );
}

export function SettingsIcon({ size = 15, className }: IconProps) {
  return (
    <svg {...base(size, className)}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.2 14.4a1.7 1.7 0 0 0 .3 1.9l.1.1a2 2 0 1 1-2.9 2.9l-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.5v.2a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.9-2.9l.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.5-1H2.4a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.6-1.1 1.7 1.7 0 0 0-.3-1.9l-.1-.1a2 2 0 1 1 2.9-2.9l.1.1a1.7 1.7 0 0 0 1.9.3h.1a1.7 1.7 0 0 0 1-1.5V2.4a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.9-.3l.1-.1a2 2 0 1 1 2.9 2.9l-.1.1a1.7 1.7 0 0 0-.3 1.9v.1a1.7 1.7 0 0 0 1.5 1h.2a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.6 1Z" />
    </svg>
  );
}

export function ChevronLeftIcon({ size = 15, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={2}>
      <path d="M14.5 5 7.5 12l7 7" />
    </svg>
  );
}

export function ChevronRightIcon({ size = 15, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={2}>
      <path d="M9.5 5l7 7-7 7" />
    </svg>
  );
}

export function CheckIcon({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={2.4}>
      <path d="M4.5 12.4 9.4 17 19.5 6.8" />
    </svg>
  );
}

export function BellIcon({ size = 15, className }: IconProps) {
  return (
    <svg {...base(size, className)}>
      <path d="M18 8.6a6 6 0 1 0-12 0c0 6-2.4 7.6-2.4 7.6h16.8S18 14.6 18 8.6" />
      <path d="M13.7 20a2 2 0 0 1-3.4 0" />
    </svg>
  );
}

export function AlertIcon({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size, className)}>
      <path d="M10.3 3.6 1.9 18a2 2 0 0 0 1.7 3h16.8a2 2 0 0 0 1.7-3L13.7 3.6a2 2 0 0 0-3.4 0Z" />
      <path d="M12 9.4v4.2" />
      <circle cx="12" cy="17.4" r="0.9" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function ClockIcon({ size = 12, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={2}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5.3l3.4 2" />
    </svg>
  );
}

export function PinIcon({ size = 12, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={1.9}>
      <path d="m8.2 3.2 7.6 7.6M10 2l7.9 7.9-2.7 1.3-2.8 4.5-1.6-1.6-1.6-1.6-4.5 2.8L2 12.6 10 2Z" />
      <path d="m8.7 15.3-4.9 4.9" />
    </svg>
  );
}

export function EmptyBoxIcon({ size = 28, className }: IconProps) {
  return (
    <svg {...base(size, className)} strokeWidth={1.4}>
      <path d="M3.2 7.6 12 3l8.8 4.6v8.8L12 21l-8.8-4.6z" />
      <path d="M3.4 7.7 12 12.2l8.6-4.5M12 12.2V21" />
    </svg>
  );
}
