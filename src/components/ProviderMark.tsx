import { useId } from "react";

import type { ProviderId } from "../types/quota";

/**
 * Hand-drawn provider marks.
 *
 * These are original simplified glyphs (a radial burst, an interlocked knot, a
 * routing fork and a spark) rather than copies of any vendor asset, so the app
 * stays shippable as open source while still being instantly recognisable.
 */

export interface ProviderMarkProps {
  provider: ProviderId;
  size?: number;
  className?: string;
}

const CLAUDE_SPOKES = Array.from({ length: 12 }, (_, index) => {
  const angle = (index * 30 * Math.PI) / 180;
  const inner = 2.4;
  const outer = index % 2 === 0 ? 9.4 : 7.4;
  return {
    x1: 12 + Math.cos(angle) * inner,
    y1: 12 + Math.sin(angle) * inner,
    x2: 12 + Math.cos(angle) * outer,
    y2: 12 + Math.sin(angle) * outer,
    width: index % 2 === 0 ? 2.1 : 1.5,
  };
});

export function ProviderMark({ provider, size = 16, className }: ProviderMarkProps) {
  const gradientId = useId();

  const common = {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    className,
    "aria-hidden": true,
    focusable: false,
  } as const;

  switch (provider) {
    case "claude":
      return (
        <svg {...common}>
          <g stroke="currentColor" strokeLinecap="round">
            {CLAUDE_SPOKES.map((spoke, index) => (
              <line
                key={index}
                x1={spoke.x1}
                y1={spoke.y1}
                x2={spoke.x2}
                y2={spoke.y2}
                strokeWidth={spoke.width}
              />
            ))}
          </g>
        </svg>
      );

    case "openai":
      return (
        <svg {...common}>
          <g
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinejoin="round"
            strokeLinecap="round"
          >
            {[0, 60, 120].map((rotation) => (
              <ellipse
                key={rotation}
                cx="12"
                cy="12"
                rx="8.4"
                ry="3.6"
                transform={`rotate(${rotation} 12 12)`}
              />
            ))}
            <circle cx="12" cy="12" r="1.5" fill="currentColor" stroke="none" />
          </g>
        </svg>
      );

    case "openrouter":
      return (
        <svg {...common}>
          <g fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
            <path d="M3 12h3.5c1.6 0 2.4-2.6 4-2.6H16" />
            <path d="M3 12h3.5c1.6 0 2.4 2.6 4 2.6H16" />
            <circle cx="3.4" cy="12" r="1.6" fill="currentColor" stroke="none" />
          </g>
          <path
            d="M15.2 6.4 21 9.4l-5.8 3z"
            fill="currentColor"
            transform="translate(0 0.05)"
          />
          <path d="M15.2 11.6 21 14.6l-5.8 3z" fill="currentColor" />
        </svg>
      );

    case "gemini":
    default:
      return (
        <svg {...common}>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
              <stop offset="0%" stopColor="#4b8dff" />
              <stop offset="42%" stopColor="#8a6ff0" />
              <stop offset="74%" stopColor="#e06a8b" />
              <stop offset="100%" stopColor="#f5b23c" />
            </linearGradient>
          </defs>
          <path
            d="M12 1.8c.5 5.1 4.3 9 9.4 9.4v1.6c-5.1.4-8.9 4.3-9.4 9.4h-1.6c-.5-5.1-4.3-9-9.4-9.4v-1.6c5.1-.4 8.9-4.3 9.4-9.4z"
            fill={`url(#${gradientId})`}
          />
        </svg>
      );
  }
}
