import { Link, Outlet } from "@tanstack/react-router";
import { ErrorBoundary } from "./components/States";
import { Shortcut } from "./components/Shortcut";

const destinations = [
  { label: "Search", to: "/" },
  { label: "Add", to: "/add" },
  { label: "Research", to: "/research" },
  { label: "Settings", to: "/settings" },
] as const;

export function App() {
  return (
    <ErrorBoundary>
      <div className="app-shell">
        <aside className="app-sidebar">
          <Link aria-label="Kosh home" className="app-brand" to="/">
            <span aria-hidden="true">K</span>
            <strong>Kosh</strong>
          </Link>
          <nav aria-label="Primary">
            {destinations.map((destination) => (
              <Link
                activeOptions={{ exact: destination.to === "/" }}
                activeProps={{ "aria-current": "page", className: "app-nav-link--active" }}
                className="app-nav-link"
                key={destination.to}
                to={destination.to}
              >
                <span aria-hidden="true" />
                {destination.label}
              </Link>
            ))}
          </nav>
          <p className="app-sidebar__hint">
            Quick add
            <Shortcut keys={["⌃", "⌥", "⌘", "K"]} label="Control Option Command K" />
          </p>
        </aside>
        <div className="app-content">
          <Outlet />
        </div>
      </div>
    </ErrorBoundary>
  );
}
