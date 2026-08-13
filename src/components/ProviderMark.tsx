import claudeMark from "../assets/providers/claude.svg";
import geminiMark from "../assets/providers/gemini.svg";
import openaiMark from "../assets/providers/openai.svg";
import openrouterMark from "../assets/providers/openrouter.svg";

import type { ProviderId } from "../types/quota";

export interface ProviderMarkProps {
  provider: ProviderId;
  size?: number;
  className?: string;
}

/**
 * Official provider marks, used only to identify the corresponding service.
 *
 * Sources:
 * - Claude: https://www.anthropic.com/press-kit
 * - OpenAI: https://developers.openai.com/favicon.svg
 * - OpenRouter: https://openrouter.ai/blog/brand/openrouter-glyph-light.svg
 * - Gemini: https://www.gstatic.com/lamda/images/gemini_sparkle_aurora_33f86dc0c0257da337c63.svg
 */
const PROVIDER_MARKS: Record<ProviderId, string> = {
  claude: claudeMark,
  openai: openaiMark,
  openrouter: openrouterMark,
  gemini: geminiMark,
};

export function ProviderMark({ provider, size = 16, className }: ProviderMarkProps) {
  return (
    <img
      src={PROVIDER_MARKS[provider]}
      width={size}
      height={size}
      className={className}
      alt=""
      aria-hidden="true"
      draggable={false}
    />
  );
}
