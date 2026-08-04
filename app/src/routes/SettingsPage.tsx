import { useState } from "react";
import {
  DEFAULT_KEYBOARD_BINDINGS,
  DEFAULT_MAIN_WINDOW_ACCELERATOR,
  KoshCommand,
  type KeyboardBinding,
} from "../backend/contracts";
import { useAppearance } from "../components/Appearance";
import { Button } from "../components/Button";
import { Select } from "../components/Select";
import { ShortcutRecorder } from "../components/ShortcutRecorder";
import { Status } from "../components/Status";
import { Toggle } from "../components/Toggle";
import { bindingFor, useShortcutSettings } from "../shortcuts/context";
import {
  DEFAULT_COPY_EXACT_NOTE_LINK_ACCELERATOR,
  DEFAULT_COPY_NOTE_LINK_ACCELERATOR,
  DEFAULT_DELETE_NOTE_ACCELERATOR,
  LocalShortcutCommand,
  localBindingFor,
} from "../shortcuts/localShortcuts";
import { BackupSettings } from "./BackupSettings";
import { SettingsDiagnostics } from "./SettingsDiagnostics";

const appearanceOptions = [
  { label: "System", value: "SYSTEM" },
  { label: "Light", value: "LIGHT" },
  { label: "Dark", value: "DARK" },
] as const;

export function SettingsPage() {
  const { appearance, setAppearance } = useAppearance();
  const [shortcutResetToken, setShortcutResetToken] = useState(0);
  const {
    error,
    loading,
    localBindings,
    resetBindings,
    settings,
    update,
    updateAutomaticChecks,
    updateLocalBinding,
  } = useShortcutSettings();
  const bindings = settings?.keyboardBindings ?? DEFAULT_KEYBOARD_BINDINGS;

  const setBinding = (command: KeyboardBinding["command"], accelerator: string) => {
    if (!settings) return;
    const keyboardBindings = settings.keyboardBindings.map((binding) =>
      binding.command === command ? { ...binding, accelerator } : binding,
    );
    void update({
      expectedRevision: settings.revision,
      keyboardBindings,
    }).catch(() => undefined);
  };

  const resetShortcutBindings = async () => {
    try {
      await resetBindings();
      setShortcutResetToken((value) => value + 1);
    } catch {
      // The settings provider keeps the actionable error visible.
    }
  };

  return (
    <main className="page page--narrow">
      <header className="page-header">
        <div>
          <p className="page-kicker">Local preferences</p>
          <h1>Settings</h1>
          <p>Keep the interface quiet and the evidence visible.</p>
        </div>
        <Status tone={error ? "danger" : "success"}>
          {error ? "Settings need attention" : loading ? "Loading…" : "Saved locally"}
        </Status>
      </header>
      <section className="settings-list">
        <label>
          <span>
            <strong>Appearance</strong>
            <small>Follow macOS or choose a fixed palette.</small>
          </span>
          <Select
            aria-label="Appearance"
            onValueChange={setAppearance}
            options={appearanceOptions}
            value={appearance}
          />
        </label>
        <label>
          <span>
            <strong>Delete note shortcut</strong>
            <small>Open a confirmation before deleting the current note.</small>
          </span>
          <ShortcutRecorder
            accelerator={
              localBindingFor(localBindings, LocalShortcutCommand.DeleteNote)?.accelerator ??
              DEFAULT_DELETE_NOTE_ACCELERATOR
            }
            disabled={loading || !settings}
            label="Delete note shortcut"
            onCapture={(accelerator) =>
              updateLocalBinding(LocalShortcutCommand.DeleteNote, accelerator)
            }
            resetToken={shortcutResetToken}
          />
        </label>
        <label>
          <span>
            <strong>Copy note link shortcut</strong>
            <small>Copy the current note URL without search-result parameters.</small>
          </span>
          <ShortcutRecorder
            accelerator={
              localBindingFor(localBindings, LocalShortcutCommand.CopyNoteLink)?.accelerator ??
              DEFAULT_COPY_NOTE_LINK_ACCELERATOR
            }
            disabled={loading || !settings}
            label="Copy note link shortcut"
            onCapture={(accelerator) =>
              updateLocalBinding(LocalShortcutCommand.CopyNoteLink, accelerator)
            }
            resetToken={shortcutResetToken}
          />
        </label>
        <label>
          <span>
            <strong>Copy exact link shortcut</strong>
            <small>Copy the complete note URL, including search-result parameters.</small>
          </span>
          <ShortcutRecorder
            accelerator={
              localBindingFor(localBindings, LocalShortcutCommand.CopyExactNoteLink)?.accelerator ??
              DEFAULT_COPY_EXACT_NOTE_LINK_ACCELERATOR
            }
            disabled={loading || !settings}
            label="Copy exact link shortcut"
            onCapture={(accelerator) =>
              updateLocalBinding(LocalShortcutCommand.CopyExactNoteLink, accelerator)
            }
            resetToken={shortcutResetToken}
          />
        </label>
        <label>
          <span>
            <strong>Automatically check for updates</strong>
            <small>
              Ask GitHub shortly after launch and every 6 hours. Manual checks remain available.
            </small>
          </span>
          <Toggle
            checked={settings?.automaticUpdateChecksEnabled ?? true}
            disabled={loading || !settings}
            label="Automatically check for updates"
            onChange={(enabled) => void updateAutomaticChecks(enabled).catch(() => undefined)}
          />
        </label>
        <label>
          <span>
            <strong>Main window shortcut</strong>
            <small>Bring Kosh forward without opening another window.</small>
          </span>
          <ShortcutRecorder
            accelerator={
              bindingFor(bindings, KoshCommand.MainWindow)?.accelerator ??
              DEFAULT_MAIN_WINDOW_ACCELERATOR
            }
            disabled={loading || !settings}
            label="Main window shortcut"
            onCapture={(accelerator) => setBinding(KoshCommand.MainWindow, accelerator)}
            resetToken={shortcutResetToken}
          />
        </label>
        {Boolean(error || settings?.shortcutErrors.length) && (
          <div className="settings-list__error" role="alert">
            {error ?? settings?.shortcutErrors.join(" ")}
          </div>
        )}
        <Button
          className="settings-list__reset"
          disabled={loading || !settings}
          onClick={() => void resetShortcutBindings()}
          size="compact"
          variant="ghost"
        >
          Reset shortcuts
        </Button>
      </section>
      <BackupSettings />
      <SettingsDiagnostics />
    </main>
  );
}
