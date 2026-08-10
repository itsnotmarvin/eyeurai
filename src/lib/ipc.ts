import type {
  LocalUsageDaySummary,
  LocalUsageModelSummary,
  LocalUsageProviderId,
  LocalUsageProviderSummary,
  LocalUsageSnapshot,
  Snapshot,
} from "../types/quota";
import { normalizeSnapshot } from "./normalize";

/**
 * Thin Tauri IPC client.
 *
 * Everything here degrades gracefully: when the app is served by plain Vite
 * (no Tauri runtime) every call resolves to `null` and callers fall back to the
 * deterministic demo snapshot. Command names are probed from a small candidate
 * list so the frontend keeps working if the Rust side picks a different verb.
 */

const SNAPSHOT_COMMANDS = ["get_snapshot", "snapshot", "quota_snapshot"] as const;
const REFRESH_COMMANDS = ["refresh_quotas", "refresh_now", "refresh"] as const;
const SNAPSHOT_EVENTS = ["snapshot-updated", "quota://snapshot"] as const;
const REFRESH_REQUEST_EVENT = "eyeurai://refresh-requested";
const LOCAL_USAGE_COMMANDS = ["scan_local_usage"] as const;
const TRAY_DISPLAY_COMMANDS = ["set_tray_display"] as const;

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type UnlistenFn = () => void;

let invokeCache: InvokeFn | null | undefined;
const resolvedCommand = new Map<string, string>();

export function isTauri(): boolean {
  if (typeof window === "undefined") return false;
  return "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
}

async function getInvoke(): Promise<InvokeFn | null> {
  if (invokeCache !== undefined) return invokeCache;
  if (!isTauri()) {
    invokeCache = null;
    return null;
  }
  try {
    const core = await import("@tauri-apps/api/core");
    invokeCache = core.invoke as InvokeFn;
  } catch {
    invokeCache = null;
  }
  return invokeCache;
}

/** Calls the first candidate command the backend actually implements. */
async function invokeAny<T>(
  key: string,
  candidates: readonly string[],
  args?: Record<string, unknown>,
): Promise<T | null> {
  const invoke = await getInvoke();
  if (!invoke) return null;

  const known = resolvedCommand.get(key);
  const order = known ? [known, ...candidates.filter((c) => c !== known)] : candidates;

  let lastError: unknown = null;
  for (const command of order) {
    try {
      const result = await invoke<T>(command, args);
      resolvedCommand.set(key, command);
      return result;
    } catch (error) {
      lastError = error;
    }
  }
  if (lastError) throw lastError;
  return null;
}

/** Reads the current snapshot. Returns `null` outside Tauri. */
export async function fetchSnapshot(excludedAccountIds: readonly string[] = []): Promise<Snapshot | null> {
  const raw = await invokeAny<unknown>("snapshot", SNAPSHOT_COMMANDS, {
    excludedAccountIds: [...excludedAccountIds],
  });
  if (raw === null || raw === undefined) return null;
  return normalizeSnapshot(raw);
}

/** Asks the backend to re-read every provider now. */
export async function requestRefresh(
  excludedAccountIds: readonly string[] = [],
): Promise<Snapshot | null> {
  const raw = await invokeAny<unknown>("refresh", REFRESH_COMMANDS, {
    excludedAccountIds: [...excludedAccountIds],
  });
  if (raw === null || raw === undefined) return null;
  return normalizeSnapshot(raw);
}

function safeCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : 0;
}

function localProvider(value: unknown): LocalUsageProviderId | null {
  return value === "claude" || value === "openai" ? value : null;
}

function normalizeLocalProvider(value: unknown): LocalUsageProviderSummary | null {
  if (typeof value !== "object" || value === null) return null;
  const row = value as Record<string, unknown>;
  const provider = localProvider(row.provider);
  if (!provider) return null;
  return {
    provider,
    processedTokens: safeCount(row.processedTokens),
    uncachedInputTokens: safeCount(row.uncachedInputTokens),
    cachedInputTokens: safeCount(row.cachedInputTokens),
    cacheWriteInputTokens: safeCount(row.cacheWriteInputTokens),
    outputTokens: safeCount(row.outputTokens),
    reasoningOutputTokens: safeCount(row.reasoningOutputTokens),
    observations: safeCount(row.observations),
    sessions: safeCount(row.sessions),
  };
}

function normalizeLocalDay(value: unknown): LocalUsageDaySummary | null {
  if (typeof value !== "object" || value === null) return null;
  const row = value as Record<string, unknown>;
  const provider = localProvider(row.provider);
  const date = typeof row.date === "string" && /^\d{4}-\d{2}-\d{2}$/.test(row.date)
    ? row.date
    : null;
  if (!provider || !date) return null;
  return { date, provider, processedTokens: safeCount(row.processedTokens) };
}

function normalizeLocalModel(value: unknown): LocalUsageModelSummary | null {
  if (typeof value !== "object" || value === null) return null;
  const row = value as Record<string, unknown>;
  const provider = localProvider(row.provider);
  const model = typeof row.model === "string" ? row.model.trim().slice(0, 80) : "";
  if (!provider || !model) return null;
  return { provider, model, processedTokens: safeCount(row.processedTokens) };
}

function normalizeLocalUsage(value: unknown): LocalUsageSnapshot | null {
  if (typeof value !== "object" || value === null) return null;
  const row = value as Record<string, unknown>;
  const generatedAt = typeof row.generatedAt === "string" ? row.generatedAt : null;
  if (!generatedAt || !Array.isArray(row.providers)) return null;
  const daily = Array.isArray(row.daily) ? row.daily : [];
  const models = Array.isArray(row.models) ? row.models : [];
  return {
    generatedAt,
    rangeDays: Math.max(1, Math.min(90, safeCount(row.rangeDays) || 7)),
    processedTokens: safeCount(row.processedTokens),
    uncachedInputTokens: safeCount(row.uncachedInputTokens),
    cachedInputTokens: safeCount(row.cachedInputTokens),
    cacheWriteInputTokens: safeCount(row.cacheWriteInputTokens),
    outputTokens: safeCount(row.outputTokens),
    reasoningOutputTokens: safeCount(row.reasoningOutputTokens),
    observations: safeCount(row.observations),
    sessions: safeCount(row.sessions),
    providers: row.providers
      .map(normalizeLocalProvider)
      .filter((provider): provider is LocalUsageProviderSummary => provider !== null),
    daily: daily.map(normalizeLocalDay).filter((day): day is LocalUsageDaySummary => day !== null),
    models: models
      .map(normalizeLocalModel)
      .filter((model): model is LocalUsageModelSummary => model !== null),
  };
}

/** Reads aggregate counters from local CLI logs after the user opts in. */
export async function fetchLocalUsage(days: number = 7): Promise<LocalUsageSnapshot | null> {
  const raw = await invokeAny<unknown>("local-usage", LOCAL_USAGE_COMMANDS, { days });
  return raw === null || raw === undefined ? null : normalizeLocalUsage(raw);
}

/** Switches the native menu-bar item between the EyeUrAI mark and one live quota label. */
export async function setTrayDisplay(
  windowLabel: string | null,
  percentUsed: number | null,
): Promise<void> {
  await invokeAny<unknown>("tray-display", TRAY_DISPLAY_COMMANDS, {
    windowLabel,
    percentUsed,
  });
}

/** Subscribes to backend-pushed snapshots. No-op outside Tauri. */
export async function subscribeToSnapshots(
  onSnapshot: (snapshot: Snapshot) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  try {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisteners = await Promise.all(
      SNAPSHOT_EVENTS.map((event) =>
        listen<unknown>(event, (message) => {
          const snapshot = normalizeSnapshot(message.payload);
          if (snapshot) onSnapshot(snapshot);
        }),
      ),
    );
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  } catch {
    return () => {};
  }
}

/** Handles the native tray menu's “Refresh limits” item. */
export async function subscribeToRefreshRequests(onRefresh: () => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen(REFRESH_REQUEST_EVENT, onRefresh);
  } catch {
    return () => {};
  }
}

/** Hides the popover window (used by the Escape shortcut). */
export async function hideWindow(): Promise<void> {
  if (!isTauri()) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().hide();
  } catch {
    /* window controls are best effort */
  }
}

/** Test seam: forget the cached runtime handles. */
export function resetIpcCacheForTests(): void {
  invokeCache = undefined;
  resolvedCommand.clear();
}
