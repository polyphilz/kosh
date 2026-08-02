import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { TauriCommand } from "../tauriProtocol.ts";
import type { UpdateGateway } from "./contracts.ts";

let pendingUpdate: Update | null = null;
let updateCheckTail: Promise<void> = Promise.resolve();
const UPDATE_CHECK_TIMEOUT_MS = 30_000;
const UPDATE_DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1_000;

function serializeUpdateCheck<T>(operation: () => Promise<T>): Promise<T> {
  const result = updateCheckTail.then(operation);
  updateCheckTail = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}

export const tauriUpdateGateway: UpdateGateway = {
  async check() {
    return serializeUpdateCheck(async () => {
      await pendingUpdate?.close();
      pendingUpdate = await check({ timeout: UPDATE_CHECK_TIMEOUT_MS });
      if (pendingUpdate === null) {
        return null;
      }
      return {
        currentVersion: pendingUpdate.currentVersion,
        version: pendingUpdate.version,
        notes: pendingUpdate.body ?? null,
        publishedAt: pendingUpdate.date ?? null,
      };
    });
  },
  async downloadAndInstall(onProgress) {
    if (pendingUpdate === null) {
      throw new Error("No checked update is available to install");
    }
    let downloadedBytes = 0;
    let totalBytes: number | null = null;
    await pendingUpdate.downloadAndInstall(
      (event: DownloadEvent) => {
        switch (event.event) {
          case "Started":
            totalBytes = event.data.contentLength ?? null;
            onProgress({ downloadedBytes, totalBytes });
            break;
          case "Progress":
            downloadedBytes += event.data.chunkLength;
            onProgress({ downloadedBytes, totalBytes });
            break;
          case "Finished":
            if (totalBytes !== null) {
              downloadedBytes = totalBytes;
            }
            onProgress({ downloadedBytes, totalBytes });
            break;
        }
      },
      { timeout: UPDATE_DOWNLOAD_TIMEOUT_MS },
    );
  },
  async relaunch() {
    await invoke(TauriCommand.PrepareUpdateRelaunch);
    await relaunch();
  },
};
