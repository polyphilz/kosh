import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { Status } from "../components/Status";

export function AddPage() {
  return (
    <main className="page page--narrow">
      <header className="page-header">
        <div>
          <p className="page-kicker">Loose by design</p>
          <h1>Add a tidbit</h1>
          <p>A shower thought or a chapter of notes both belong here.</p>
        </div>
        <Status>Draft stays local</Status>
      </header>
      <form className="capture-card" onSubmit={(event) => event.preventDefault()}>
        <label htmlFor="tidbit-title">
          Title <span>optional</span>
        </label>
        <Input id="tidbit-title" placeholder="A useful handle" />
        <label htmlFor="tidbit-body">Tidbit</label>
        <textarea
          className="kosh-textarea"
          id="tidbit-body"
          placeholder="Drop the knowledge here…"
          rows={12}
        />
        <label htmlFor="tidbit-source">
          Source URL <span>optional</span>
        </label>
        <Input id="tidbit-source" placeholder="https://…" type="url" />
        <footer>
          <Button variant="ghost">Attach</Button>
          <Button type="submit" variant="accent">
            Save tidbit
          </Button>
        </footer>
      </form>
    </main>
  );
}
