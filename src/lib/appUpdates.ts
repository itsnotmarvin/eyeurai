import { isTauri } from "./ipc";

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

/** Checks the signed release feed. Browser previews and development builds never make a request. */
export async function checkForAppUpdate(): Promise<AppUpdateInfo | null> {
  if (!isTauri() || import.meta.env.DEV) return null;
  if (pendingUpdate) return updateInfo(pendingUpdate);

  const { check } = await import("@tauri-apps/plugin-updater");
  const update = await check({ timeout: 15_000 });
  if (!update) return null;
  pendingUpdate = update;
  return updateInfo(update);
}

/** Downloads, verifies, and installs the pending update, then relaunches EyeUrAI. */
export async function installAppUpdate(
  onProgress: (progress: AppUpdateProgress) => void,
): Promise<void> {
  if (!pendingUpdate) throw new Error("No EyeUrAI update is ready to install.");

  let downloaded = 0;
  let total: number | null = null;
  await pendingUpdate.downloadAndInstall(
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

  onProgress({ stage: "installing", percent: 100 });
  const { relaunch } = await import("@tauri-apps/plugin-process");
  await relaunch();
}

/** Test seam for releasing the native updater resource between tests. */
export async function resetAppUpdateForTests(): Promise<void> {
  const update = pendingUpdate;
  pendingUpdate = null;
  if (update) await update.close();
}
