import { Button } from "../components/Button.tsx";
import { UpdateCheckOrigin, UpdatePhase, type UpdateState } from "./contracts.ts";
import type { UpdateController } from "./controller.ts";

export function UpdateNotification({
  controller,
  state,
}: {
  controller: UpdateController;
  state: UpdateState;
}) {
  if (
    state.phase === UpdatePhase.Idle ||
    (state.phase === UpdatePhase.Checking && state.origin === UpdateCheckOrigin.Automatic)
  ) {
    return null;
  }

  const content = notificationContent(state);
  const busy = state.phase === UpdatePhase.Downloading || state.phase === UpdatePhase.Installing;

  return (
    <aside
      aria-atomic="true"
      aria-live={state.phase === UpdatePhase.Error ? "assertive" : "polite"}
      className="update-notification"
      role={state.phase === UpdatePhase.Error ? "alert" : "status"}
    >
      <div>
        <strong>{content.heading}</strong>
        <span>{content.detail}</span>
      </div>
      <div className="update-notification-actions">
        {state.phase === UpdatePhase.Available && (
          <Button onClick={() => void controller.installAndRestart()} size="compact" type="button">
            Install and restart
          </Button>
        )}
        {state.phase === UpdatePhase.Error && (
          <Button onClick={() => void controller.checkManually()} size="compact" type="button">
            Try again
          </Button>
        )}
        {!busy && (
          <Button onClick={() => controller.dismiss()} size="compact" type="button">
            {state.phase === UpdatePhase.Available ? "Not now" : "Dismiss"}
          </Button>
        )}
      </div>
    </aside>
  );
}

function notificationContent(state: Exclude<UpdateState, { phase: typeof UpdatePhase.Idle }>): {
  heading: string;
  detail: string;
} {
  switch (state.phase) {
    case UpdatePhase.Checking:
      return {
        heading: "Checking for updates…",
        detail: "Kosh is asking GitHub for the latest published release.",
      };
    case UpdatePhase.Available:
      return {
        heading: `Kosh ${state.update.version} is available`,
        detail: "Install the signed update and reopen Kosh when it is ready.",
      };
    case UpdatePhase.Downloading:
      return {
        heading: `Downloading Kosh ${state.update.version}…`,
        detail: downloadProgressLabel(state.downloadedBytes, state.totalBytes),
      };
    case UpdatePhase.Installing:
      return {
        heading: "Installing update…",
        detail: "Kosh will close cleanly and reopen in a moment.",
      };
    case UpdatePhase.Current:
      return {
        heading: "Kosh is up to date",
        detail: "You have the newest published version.",
      };
    case UpdatePhase.Error:
      return {
        heading: "Could not check for updates",
        detail: state.message,
      };
  }
}

function downloadProgressLabel(downloadedBytes: number, totalBytes: number | null): string {
  if (totalBytes === null || totalBytes <= 0) {
    return `${formatBytes(downloadedBytes)} downloaded`;
  }
  const percent = Math.min(100, Math.round((downloadedBytes / totalBytes) * 100));
  return `${percent}% · ${formatBytes(downloadedBytes)} of ${formatBytes(totalBytes)}`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1_024) {
    return `${bytes} B`;
  }
  if (bytes < 1_048_576) {
    return `${Math.round(bytes / 1_024)} KB`;
  }
  if (bytes >= 1_073_741_824) {
    return `${(bytes / 1_073_741_824).toFixed(1)} GB`;
  }
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}
