export const UpdateCheckOrigin = {
  Automatic: "AUTOMATIC",
  Manual: "MANUAL",
} as const;

export type UpdateCheckOrigin = (typeof UpdateCheckOrigin)[keyof typeof UpdateCheckOrigin];

export const UpdatePhase = {
  Idle: "IDLE",
  Checking: "CHECKING",
  Available: "AVAILABLE",
  Downloading: "DOWNLOADING",
  Installing: "INSTALLING",
  Current: "CURRENT",
  Error: "ERROR",
} as const;

export type UpdatePhase = (typeof UpdatePhase)[keyof typeof UpdatePhase];

export interface AvailableUpdate {
  currentVersion: string;
  version: string;
  notes: string | null;
  publishedAt: string | null;
}

export interface UpdateDownloadProgress {
  downloadedBytes: number;
  totalBytes: number | null;
}

export interface UpdateGateway {
  check(): Promise<AvailableUpdate | null>;
  downloadAndInstall(onProgress: (progress: UpdateDownloadProgress) => void): Promise<void>;
  relaunch(): Promise<void>;
}

export function updaterIsEnabled(
  tauriEnvironment: boolean,
  releaseMarker: string | undefined,
): boolean {
  return tauriEnvironment && releaseMarker === "true";
}

export type UpdateState =
  | { phase: typeof UpdatePhase.Idle }
  | {
      phase: typeof UpdatePhase.Checking;
      origin: UpdateCheckOrigin;
    }
  | {
      phase: typeof UpdatePhase.Available;
      update: AvailableUpdate;
    }
  | {
      phase: typeof UpdatePhase.Downloading;
      update: AvailableUpdate;
      downloadedBytes: number;
      totalBytes: number | null;
    }
  | {
      phase: typeof UpdatePhase.Installing;
      update: AvailableUpdate;
    }
  | { phase: typeof UpdatePhase.Current }
  | {
      phase: typeof UpdatePhase.Error;
      message: string;
    };
