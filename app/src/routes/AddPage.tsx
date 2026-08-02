import { useNavigate } from "@tanstack/react-router";
import { Status } from "../components/Status";
import { TidbitComposer } from "./TidbitComposer";

export function AddPage() {
  const navigate = useNavigate();
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
      <TidbitComposer
        onCancel={() => void navigate({ to: "/search" })}
        onSaved={(tidbit) =>
          navigate({
            to: "/tidbits/$tidbitId",
            params: { tidbitId: tidbit.id },
          })
        }
      />
    </main>
  );
}
