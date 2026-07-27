import { useState } from "react";
import { useAppearance } from "../components/Appearance";
import { Select } from "../components/Select";
import { Status } from "../components/Status";
import { Toggle } from "../components/Toggle";

const appearanceOptions = [
  { label: "System", value: "SYSTEM" },
  { label: "Light", value: "LIGHT" },
  { label: "Dark", value: "DARK" },
] as const;

export function SettingsPage() {
  const { appearance, setAppearance } = useAppearance();
  const [citationPreview, setCitationPreview] = useState(true);

  return (
    <main className="page page--narrow">
      <header className="page-header">
        <div>
          <p className="page-kicker">Local preferences</p>
          <h1>Settings</h1>
          <p>Keep the interface quiet and the evidence visible.</p>
        </div>
        <Status tone="success">Saved locally</Status>
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
            <strong>Citation previews</strong>
            <small>Show the cited passage beside result metadata.</small>
          </span>
          <Toggle
            checked={citationPreview}
            label="Citation previews"
            onChange={setCitationPreview}
          />
        </label>
      </section>
    </main>
  );
}
