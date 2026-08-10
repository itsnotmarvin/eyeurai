import { useMemo, useState } from "react";

import { formatQuantity } from "../lib/format";
import type {
  LocalUsageDaySummary,
  LocalUsageProviderId,
  LocalUsageSnapshot,
} from "../types/quota";
import { ProviderMark } from "./ProviderMark";

type RangeDays = 7 | 30 | 90;
type BreakdownMode = "model" | "day";

export interface LocalUsagePanelProps {
  enabled: boolean;
  usage: LocalUsageSnapshot | null;
  loading: boolean;
  error: string | null;
  rangeDays: RangeDays;
  onRangeChange: (days: RangeDays) => void;
  onReviewAccess: () => void;
}

const PROVIDER_NAME: Record<LocalUsageProviderId, string> = {
  openai: "Codex",
  claude: "Claude Code",
};

const CHART_WIDTH = 380;
const CHART_HEIGHT = 132;
const CHART_LEFT = 36;
const CHART_RIGHT = 5;
const CHART_TOP = 8;
const CHART_BOTTOM = 17;

function tokens(value: number): string {
  return formatQuantity(value, "tokens");
}

function parseDate(date: string): Date {
  return new Date(`${date}T12:00:00Z`);
}

function shortDate(date: string): string {
  return new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", timeZone: "UTC" })
    .format(parseDate(date));
}

function rangeCaption(usage: LocalUsageSnapshot): string {
  const end = new Date(usage.generatedAt);
  const start = new Date(end);
  start.setUTCDate(start.getUTCDate() - usage.rangeDays + 1);
  const formatter = new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
  return `${formatter.format(start)} – ${formatter.format(end)}`;
}

function chartDates(usage: LocalUsageSnapshot): string[] {
  const end = new Date(usage.generatedAt);
  end.setUTCHours(12, 0, 0, 0);
  return Array.from({ length: usage.rangeDays }, (_, index) => {
    const date = new Date(end);
    date.setUTCDate(end.getUTCDate() - (usage.rangeDays - 1 - index));
    return date.toISOString().slice(0, 10);
  });
}

function chartSeries(usage: LocalUsageSnapshot) {
  const dates = chartDates(usage);
  const observed = new Map<string, number>();
  for (const day of usage.daily) {
    observed.set(`${day.provider}:${day.date}`, day.processedTokens);
  }
  const values = (["openai", "claude"] as const).map((provider) => ({
    provider,
    values: dates.map((date) => observed.get(`${provider}:${date}`) ?? 0),
  }));
  return { dates, values };
}

function points(values: number[], max: number): string {
  const drawableWidth = CHART_WIDTH - CHART_LEFT - CHART_RIGHT;
  const drawableHeight = CHART_HEIGHT - CHART_TOP - CHART_BOTTOM;
  return values
    .map((value, index) => {
      const x =
        CHART_LEFT + (values.length === 1 ? drawableWidth / 2 : (index / (values.length - 1)) * drawableWidth);
      const y = CHART_TOP + drawableHeight - (value / max) * drawableHeight;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

function combinedDays(daily: LocalUsageDaySummary[]): Array<{ date: string; processedTokens: number }> {
  const rows = new Map<string, number>();
  for (const item of daily) {
    rows.set(item.date, (rows.get(item.date) ?? 0) + item.processedTokens);
  }
  return [...rows.entries()]
    .map(([date, processedTokens]) => ({ date, processedTokens }))
    .sort((left, right) => right.date.localeCompare(left.date));
}

export function LocalUsagePanel({
  enabled,
  usage,
  loading,
  error,
  rangeDays,
  onRangeChange,
  onReviewAccess,
}: LocalUsagePanelProps) {
  const [breakdownMode, setBreakdownMode] = useState<BreakdownMode>("model");
  const chart = useMemo(() => (usage ? chartSeries(usage) : null), [usage]);

  return (
    <section className="localusage" aria-label="Local token usage" data-updating={loading || undefined}>
      <div className="localusage__head">
        <div>
          <h2 className="localusage__title">Usage</h2>
          <p className="localusage__eyebrow">
            {usage ? rangeCaption(usage) : `This Mac · last ${rangeDays} days`}
          </p>
        </div>
        <span className="localusage__local">private · this Mac</span>
      </div>

      {!enabled ? (
        <div className="localusage__permission">
          <p>
            Add T3-style token trends and breakdowns from local Claude Code and Codex logs.
            Nothing is uploaded.
          </p>
          <button type="button" className="btn btn--primary" onClick={onReviewAccess}>
            Review access
          </button>
        </div>
      ) : error ? (
        <div className="localusage__permission">
          <p>{error}</p>
          <button type="button" className="btn btn--ghost" onClick={onReviewAccess}>
            Manage access
          </button>
        </div>
      ) : loading && !usage ? (
        <p className="localusage__state" role="status">Reading local counters…</p>
      ) : !usage || usage.observations === 0 ? (
        <p className="localusage__state">
          No recent Claude Code or Codex token counters were found on this Mac.
        </p>
      ) : (
        <>
          <div className="localusage__range" aria-label="Usage range">
            {([7, 30, 90] as const).map((days) => (
              <button
                key={days}
                type="button"
                aria-pressed={rangeDays === days}
                onClick={() => onRangeChange(days)}
              >
                {days} days
              </button>
            ))}
          </div>

          <div className="localusage__hero">
            <span className="localusage__heroLabel">Processed tokens</span>
            <strong>{tokens(usage.processedTokens)}</strong>
            <span>{usage.sessions} sessions</span>
          </div>

          <div className="localusage__providers">
            {[...usage.providers]
              .sort((left, right) => right.processedTokens - left.processedTokens)
              .map((provider) => {
                const share =
                  usage.processedTokens > 0
                    ? (provider.processedTokens / usage.processedTokens) * 100
                    : 0;
                return (
                  <div
                    className="localusage__provider"
                    data-provider={provider.provider}
                    key={provider.provider}
                  >
                    <div className="localusage__providerLabel">
                      <ProviderMark provider={provider.provider} size={15} />
                      <span>{PROVIDER_NAME[provider.provider]}</span>
                      <strong>{tokens(provider.processedTokens)}</strong>
                    </div>
                    <span className="localusage__track" aria-hidden="true">
                      <span style={{ width: `${Math.max(1.5, share)}%` }} />
                    </span>
                    <span className="localusage__providerMeta">
                      {share.toFixed(1)}% of tokens · {provider.sessions} sessions
                    </span>
                  </div>
                );
              })}
          </div>

          {chart ? (
            <div className="localusage__chartBlock">
              <div className="localusage__chartHead">
                <h3>Daily tokens</h3>
                <div className="localusage__legend" aria-label="Chart legend">
                  <span data-provider="openai"><i />Codex</span>
                  <span data-provider="claude"><i />Claude Code</span>
                </div>
              </div>
              {(() => {
                const max = Math.max(1, ...chart.values.flatMap((series) => series.values));
                return (
                  <svg
                    className="localusage__chart"
                    viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
                    role="img"
                    aria-label={`Daily processed tokens over ${rangeDays} days`}
                  >
                    <defs>
                      <linearGradient id="claude-area" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="0%" stopColor="#d97757" stopOpacity="0.25" />
                        <stop offset="100%" stopColor="#d97757" stopOpacity="0" />
                      </linearGradient>
                    </defs>
                    {[0, 0.5, 1].map((ratio) => {
                      const y = CHART_TOP + ratio * (CHART_HEIGHT - CHART_TOP - CHART_BOTTOM);
                      return (
                        <g key={ratio}>
                          <line x1={CHART_LEFT} x2={CHART_WIDTH - CHART_RIGHT} y1={y} y2={y} />
                          <text x={CHART_LEFT - 5} y={y + 3}>{tokens(max * (1 - ratio))}</text>
                        </g>
                      );
                    })}
                    {chart.values.map((series) => (
                      <polyline
                        key={series.provider}
                        data-provider={series.provider}
                        points={points(series.values, max)}
                      />
                    ))}
                  </svg>
                );
              })()}
              <div className="localusage__chartDates" aria-hidden="true">
                <span>{shortDate(chart.dates[0])}</span>
                <span>{shortDate(chart.dates[Math.floor(chart.dates.length / 2)])}</span>
                <span>{shortDate(chart.dates[chart.dates.length - 1])}</span>
              </div>
            </div>
          ) : null}

          <dl className="localusage__metrics">
            <div>
              <dt>Processed</dt>
              <dd>{tokens(usage.processedTokens)}</dd>
              <small>{tokens(usage.processedTokens / Math.max(1, new Set(usage.daily.map((day) => day.date)).size))} per active day</small>
            </div>
            <div>
              <dt>Cached input</dt>
              <dd>{tokens(usage.cachedInputTokens)}</dd>
              <small>{usage.processedTokens > 0 ? `${((usage.cachedInputTokens / usage.processedTokens) * 100).toFixed(1)}% of processed` : "—"}</small>
            </div>
            <div>
              <dt>Uncached input</dt>
              <dd>{tokens(usage.uncachedInputTokens)}</dd>
              <small>{tokens(usage.cacheWriteInputTokens)} cache writes</small>
            </div>
            <div>
              <dt>Output</dt>
              <dd>{tokens(usage.outputTokens)}</dd>
              <small>includes {tokens(usage.reasoningOutputTokens)} reasoning</small>
            </div>
          </dl>

          <div className="localusage__breakdown">
            <div className="localusage__breakdownHead">
              <h3>Breakdown</h3>
              <div className="localusage__segmented" aria-label="Breakdown grouping">
                {(["model", "day"] as const).map((mode) => (
                  <button
                    key={mode}
                    type="button"
                    aria-pressed={breakdownMode === mode}
                    onClick={() => setBreakdownMode(mode)}
                  >
                    {mode}
                  </button>
                ))}
              </div>
            </div>
            <div className="localusage__table" role="table" aria-label={`${breakdownMode} usage breakdown`}>
              <div className="localusage__tableHead" role="row">
                <span role="columnheader">{breakdownMode === "model" ? "Model" : "Day"}</span>
                <span role="columnheader">Share</span>
                <span role="columnheader">Tokens</span>
              </div>
              {breakdownMode === "model"
                ? usage.models.slice(0, 10).map((model) => {
                    const share = usage.processedTokens > 0
                      ? (model.processedTokens / usage.processedTokens) * 100
                      : 0;
                    return (
                      <div className="localusage__tableRow" role="row" key={`${model.provider}:${model.model}`}>
                        <span role="cell" data-provider={model.provider}>
                          <ProviderMark provider={model.provider} size={13} />
                          <b title={model.model}>{model.model}</b>
                        </span>
                        <span role="cell">{share.toFixed(1)}%</span>
                        <span role="cell">{tokens(model.processedTokens)}</span>
                      </div>
                    );
                  })
                : combinedDays(usage.daily).slice(0, 10).map((day) => {
                    const share = usage.processedTokens > 0
                      ? (day.processedTokens / usage.processedTokens) * 100
                      : 0;
                    return (
                      <div className="localusage__tableRow" role="row" key={day.date}>
                        <span role="cell"><b>{shortDate(day.date)}</b></span>
                        <span role="cell">{share.toFixed(1)}%</span>
                        <span role="cell">{tokens(day.processedTokens)}</span>
                      </div>
                    );
                  })}
            </div>
          </div>

          <p className="localusage__note">
            Device-local CLI activity only. Cost is omitted until EyeUrAI can attribute model
            pricing accurately.
          </p>
        </>
      )}
    </section>
  );
}
