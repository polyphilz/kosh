import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { EmptyState } from "../components/States";
import { Status } from "../components/Status";
import { Shortcut } from "../components/Shortcut";

export function SearchPage() {
  return (
    <main className="page">
      <header className="page-header">
        <div>
          <p className="page-kicker">Your local knowledge</p>
          <h1>Search</h1>
          <p>Find exact supporting passages across every tidbit.</p>
        </div>
        <Status tone="success">Local library ready</Status>
      </header>
      <section aria-label="Search controls" className="search-bar">
        <label className="visually-hidden" htmlFor="library-search">
          Search tidbits
        </label>
        <Input
          autoComplete="off"
          id="library-search"
          placeholder="Search a thought, phrase, or idea…"
          type="search"
        />
        <Button variant="primary">
          Search
          <Shortcut keys={["↵"]} label="Return" />
        </Button>
      </section>
      <EmptyState
        detail="Your results will lead with the exact passage and its provenance."
        title="Ask your library anything"
      />
    </main>
  );
}
