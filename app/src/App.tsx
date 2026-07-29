import { Link, Outlet, useNavigate } from "@tanstack/react-router";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { DEFAULT_QUICK_ADD_ACCELERATOR, KoshCommand } from "./backend/contracts";
import { ErrorBoundary } from "./components/States";
import { Shortcut } from "./components/Shortcut";
import { acceleratorKeys, describeAccelerator } from "./shortcuts/accelerator";
import { bindingFor, ShortcutSettingsProvider, useShortcutSettings } from "./shortcuts/context";

const OPEN_SETTINGS_EVENT = "kosh://open-settings";

const destinations = [
  { label: "Search", to: "/" },
  { label: "Add", to: "/add" },
  { label: "Research", to: "/research" },
  { label: "Settings", to: "/settings" },
] as const;

export function App() {
  return (
    <ShortcutSettingsProvider>
      <AppShell />
    </ShortcutSettingsProvider>
  );
}

function AppShell() {
  const navigate = useNavigate();
  const { settings } = useShortcutSettings();
  const quickAddAccelerator =
    bindingFor(settings?.keyboardBindings ?? [], KoshCommand.QuickAdd)?.accelerator ??
    DEFAULT_QUICK_ADD_ACCELERATOR;

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen(OPEN_SETTINGS_EVENT, () => {
      if (active) void navigate({ to: "/settings" });
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [navigate]);

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
            <Shortcut
              keys={acceleratorKeys(quickAddAccelerator)}
              label={describeAccelerator(quickAddAccelerator)}
            />
          </p>
        </aside>
        <div className="app-content">
          <Outlet />
        </div>
      </div>
    </ErrorBoundary>
  );
}
