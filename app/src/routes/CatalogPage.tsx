import { useState } from "react";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Input } from "../components/Input";
import { Select } from "../components/Select";
import { EmptyState, ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";
import { Shortcut } from "../components/Shortcut";
import { Toggle } from "../components/Toggle";
import { Tooltip } from "../components/Tooltip";

type Density = "COMPACT" | "COMFORTABLE";

const densityOptions = [
  { label: "Compact", value: "COMPACT" },
  { label: "Comfortable", value: "COMFORTABLE" },
] as const;

export function CatalogPage() {
  const [density, setDensity] = useState<Density>("COMPACT");
  const [enabled, setEnabled] = useState(true);
  const [dialogOpen, setDialogOpen] = useState(false);

  return (
    <main className="page catalog-page">
      <header className="page-header">
        <div>
          <p className="page-kicker">Visual contract</p>
          <h1>Shared primitives</h1>
          <p>Warm surfaces, compact controls, visible focus, restrained motion.</p>
        </div>
        <Status tone="success">Catalog stable</Status>
      </header>

      <section className="catalog-grid">
        <article>
          <h2>Actions</h2>
          <div className="catalog-row">
            <Button>Surface</Button>
            <Button variant="ghost">Ghost</Button>
            <Button variant="primary">Primary</Button>
            <Button variant="accent">Accent</Button>
            <Button variant="danger">Danger</Button>
          </div>
          <div className="catalog-row">
            <Button size="compact">Compact</Button>
            <Button disabled>Disabled</Button>
            <Tooltip content="Passage-level evidence" forceOpen>
              <Button aria-label="Citation help" size="icon">
                ?
              </Button>
            </Tooltip>
          </div>
        </article>

        <article>
          <h2>Fields</h2>
          <label className="catalog-field">
            <span>Search phrase</span>
            <Input defaultValue="retrieval augmented memory" />
          </label>
          <div className="catalog-row catalog-row--between">
            <Select
              aria-label="Density"
              onValueChange={setDensity}
              options={densityOptions}
              value={density}
            />
            <Toggle checked={enabled} label="Semantic search" onChange={setEnabled} />
          </div>
        </article>

        <article>
          <h2>Status and shortcuts</h2>
          <div className="catalog-stack">
            <Status>Idle</Status>
            <Status tone="success">Indexed</Status>
            <Status tone="warning">Embedding queued</Status>
            <Status tone="danger">Extraction failed</Status>
          </div>
          <div className="catalog-row">
            <Shortcut keys={["⌘", "K"]} label="Command K" />
            <Shortcut keys={["⇧", "↵"]} label="Shift Return" />
          </div>
        </article>

        <article>
          <h2>Dialog</h2>
          <p>Focus is trapped, Escape closes, and the trigger regains focus.</p>
          <Button onClick={() => setDialogOpen(true)} variant="primary">
            Open dialog
          </Button>
        </article>
      </section>

      <section aria-label="View states" className="catalog-states">
        <LoadingState detail="Building citation-sized passages…" title="Indexing" />
        <EmptyState detail="Capture something worth finding later." title="Nothing here yet" />
        <ErrorState detail="The source file moved." title="Attachment unavailable" />
      </section>

      <Dialog
        description="This sample proves focus, hierarchy, and destructive-action spacing."
        footer={
          <>
            <Button data-autofocus onClick={() => setDialogOpen(false)} variant="ghost">
              Cancel
            </Button>
            <Button onClick={() => setDialogOpen(false)} variant="danger">
              Remove
            </Button>
          </>
        }
        onClose={() => setDialogOpen(false)}
        open={dialogOpen}
        title="Remove this source?"
      >
        <p>The tidbit stays intact. Only this source reference is removed.</p>
      </Dialog>
    </main>
  );
}
