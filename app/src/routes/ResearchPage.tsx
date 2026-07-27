import { Button } from "../components/Button";
import { EmptyState } from "../components/States";
import { Status } from "../components/Status";

export function ResearchPage() {
  return (
    <main className="page">
      <header className="page-header">
        <div>
          <p className="page-kicker">Longer-haul synthesis</p>
          <h1>Research</h1>
          <p>Claude can inspect Kosh through read-only, citation-safe tools.</p>
        </div>
        <Status tone="neutral">No web access</Status>
      </header>
      <EmptyState
        action={<Button variant="primary">Start a research run</Button>}
        detail="Completed runs and their immutable citations will collect here."
        title="No research runs yet"
      />
    </main>
  );
}
