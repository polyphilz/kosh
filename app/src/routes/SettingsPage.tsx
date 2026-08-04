import { useState, type ReactNode } from "react";
import {
  DEFAULT_KEYBOARD_BINDINGS,
  DEFAULT_MAIN_WINDOW_ACCELERATOR,
  KoshCommand,
  type KeyboardBinding,
} from "../backend/contracts";
import { useAppearance } from "../components/Appearance";
import { Button } from "../components/Button";
import { KoshText } from "../components/KoshText";
import { Select } from "../components/Select";
import { ShortcutRecorder } from "../components/ShortcutRecorder";
import { Status } from "../components/Status";
import { Toggle } from "../components/Toggle";
import { KoshTextTone, KoshTextVariant } from "../components/kosh-text-types";
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
          <KoshText
            as="p"
            className="page-kicker"
            tone={KoshTextTone.Accent}
            variant={KoshTextVariant.Eyebrow}
          >
            Local preferences
          </KoshText>
          <KoshText as="h1" variant={KoshTextVariant.Title}>
            Settings
          </KoshText>
          <KoshText as="p" tone={KoshTextTone.Muted} variant={KoshTextVariant.Body}>
            Keep the interface quiet and the evidence visible.
          </KoshText>
        </div>
        <Status tone={error ? "danger" : "success"}>
          {error ? "Settings need attention" : loading ? "Loading…" : "Saved locally"}
        </Status>
      </header>
      <section className="settings-list">
        <label>
          <SettingsRowCopy
            description="Follow macOS or choose a fixed palette."
            label="Appearance"
          />
          <Select
            aria-label="Appearance"
            onValueChange={setAppearance}
            options={appearanceOptions}
            value={appearance}
          />
        </label>
        <label>
          <SettingsRowCopy
            description="Open a confirmation before deleting the current note."
            label="Delete note shortcut"
          />
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
          <SettingsRowCopy
            description="Copy the current note URL without search-result parameters."
            label="Copy note link shortcut"
          />
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
          <SettingsRowCopy
            description="Copy the complete note URL, including search-result parameters."
            label="Copy exact link shortcut"
          />
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
          <SettingsRowCopy
            description={
              <>
                Ask GitHub shortly after launch and every 6 hours. Manual checks remain available.
              </>
            }
            label="Automatically check for updates"
          />
          <Toggle
            checked={settings?.automaticUpdateChecksEnabled ?? true}
            disabled={loading || !settings}
            label="Automatically check for updates"
            onChange={(enabled) => void updateAutomaticChecks(enabled).catch(() => undefined)}
          />
        </label>
        <label>
          <SettingsRowCopy
            description="Bring Kosh forward without opening another window."
            label="Main window shortcut"
          />
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
          <KoshText
            as="div"
            className="settings-list__error"
            role="alert"
            tone={KoshTextTone.Danger}
            variant={KoshTextVariant.Supporting}
          >
            {error ?? settings?.shortcutErrors.join(" ")}
          </KoshText>
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

function SettingsRowCopy({ description, label }: { description: ReactNode; label: string }) {
  return (
    <span>
      <KoshText as="strong" variant={KoshTextVariant.Label}>
        {label}
      </KoshText>
      <KoshText as="small" tone={KoshTextTone.Muted} variant={KoshTextVariant.Supporting}>
        {description}
      </KoshText>
    </span>
  );
}
