import {
  cancelUpdateRelaunch,
  isTauri,
  prepareUpdateRelaunch,
  relaunchAfterUpdate,
} from "./ipc";

export interface AppUpdateInfo {
  currentVersion: string;
  version: string;
  notes: string | null;
}

export type AppUpdateProgress =
  | { stage: "downloading"; percent: number | null }
  | { stage: "installing"; percent: 100 };

interface PendingUpdate {
  currentVersion: string;
  version: string;
  body?: string;
  downloadAndInstall: (
    onEvent?: (event: DownloadEvent) => void,
    options?: { timeout?: number },
  ) => Promise<void>;
  close: () => Promise<void>;
}

type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

let pendingUpdate: PendingUpdate | null = null;
let installedUpdateAwaitingRelaunch = false;

function normalizedNotes(body?: string): string | null {
  const notes = body?.trim();
  return notes ? notes.slice(0, 2_000) : null;
}

function updateInfo(update: PendingUpdate): AppUpdateInfo {
  return {
    currentVersion: update.currentVersion,
    version: update.version,
    notes: normalizedNotes(update.body),
  };
}

interface ParsedVersion {
  core: [number, number, number];
  prerelease: string[];
}

function parseVersion(version: string): ParsedVersion | null {
  const match = version.trim().match(
    /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/,
  );
  if (!match) return null;
  return {
    core: [Number(match[1]), Number(match[2]), Number(match[3])],
    prerelease: match[4]?.split(".") ?? [],
  };
}

/** Returns true only when candidate is a valid semantic version newer than current. */
export function isNewerAppVersion(candidate: string, current: string): boolean {
  const next = parseVersion(candidate);
  const installed = parseVersion(current);
  if (!next || !installed) return false;

  for (let index = 0; index < next.core.length; index += 1) {
    if (next.core[index] !== installed.core[index]) {
      return next.core[index] > installed.core[index];
    }
  }

  if (next.prerelease.length === 0 || installed.prerelease.length === 0) {
    return next.prerelease.length === 0 && installed.prerelease.length > 0;
  }

  const length = Math.max(next.prerelease.length, installed.prerelease.length);
  for (let index = 0; index < length; index += 1) {
    const candidatePart = next.prerelease[index];
    const currentPart = installed.prerelease[index];
    if (candidatePart === undefined) return false;
    if (currentPart === undefined) return true;
    if (candidatePart === currentPart) continue;

    const candidateNumber = /^\d+$/.test(candidatePart) ? Number(candidatePart) : null;
    const currentNumber = /^\d+$/.test(currentPart) ? Number(currentPart) : null;
    if (candidateNumber !== null && currentNumber !== null) return candidateNumber > currentNumber;
    if (candidateNumber !== null) return false;
    if (currentNumber !== null) return true;
    return candidatePart > currentPart;
  }

  return false;
}

/** Checks the signed release feed. Browser previews and development builds never make a request. */
export async function checkForAppUpdate(): Promise<AppUpdateInfo | null> {
  if (!isTauri() || import.meta.env.DEV) return null;
  if (pendingUpdate) return updateInfo(pendingUpdate);

  const { check } = await import("@tauri-apps/plugin-updater");
  const update = await check({ timeout: 15_000 });
  if (!update) return null;
  if (!isNewerAppVersion(update.version, update.currentVersion)) {
    await update.close();
    return null;
  }
  pendingUpdate = update;
  return updateInfo(update);
}

/** Downloads, verifies, and installs the pending update, then relaunches EyeUrAI. */
export async function installAppUpdate(
  onProgress: (progress: AppUpdateProgress) => void,
): Promise<void> {
  // If installation succeeded but the readiness handshake failed, retry only
  // the relaunch. Reusing the consumed updater resource could reinstall the
  // same payload and leaves platform-specific resource state ambiguous.
  if (installedUpdateAwaitingRelaunch) {
    onProgress({ stage: "installing", percent: 100 });
    await relaunchAfterUpdate();
    installedUpdateAwaitingRelaunch = false;
    return;
  }
  if (!pendingUpdate) throw new Error("No EyeUrAI update is ready to install.");
  const update = pendingUpdate;

  let downloaded = 0;
  let total: number | null = null;
  await prepareUpdateRelaunch(update.version);
  try {
    await update.downloadAndInstall(
      (event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? null;
          onProgress({ stage: "downloading", percent: total === 0 ? 0 : null });
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          const percent = total && total > 0
            ? Math.min(99, Math.round((downloaded / total) * 100))
            : null;
          onProgress({ stage: "downloading", percent });
        } else {
          onProgress({ stage: "installing", percent: 100 });
        }
      },
      { timeout: 120_000 },
    );
  } catch (cause) {
    // Windows exits inside a successful install, so this runs only when the
    // current process remains alive and the one-use visibility marker is stale.
    try {
      await cancelUpdateRelaunch();
    } catch {
      // Preserve the actionable download/install error for the user.
    }
    throw cause;
  }

  pendingUpdate = null;
  installedUpdateAwaitingRelaunch = true;
  try {
    await update.close();
  } catch {
    // The signed update is already installed. Resource cleanup must not block
    // the only useful next action: handing off to the replacement process.
  }
  onProgress({ stage: "installing", percent: 100 });
  await relaunchAfterUpdate();
  installedUpdateAwaitingRelaunch = false;
}

/** Test seam for releasing the native updater resource between tests. */
export async function resetAppUpdateForTests(): Promise<void> {
  const update = pendingUpdate;
  pendingUpdate = null;
  installedUpdateAwaitingRelaunch = false;
  if (update) await update.close();
}

/** Test seam for exercising installation without contacting the release feed. */
export function setPendingAppUpdateForTests(update: PendingUpdate): void {
  installedUpdateAwaitingRelaunch = false;
  pendingUpdate = update;
}
