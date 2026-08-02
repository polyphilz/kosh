import { Link, Outlet, useNavigate } from "@tanstack/react-router";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { DEFAULT_QUICK_ADD_ACCELERATOR, KoshCommand } from "./backend/contracts";
import { ErrorBoundary } from "./components/States";
import { Shortcut } from "./components/Shortcut";
import { createUuidV7 } from "./notes/autosave";
import { SearchOverlay } from "./search/SearchOverlay";
import { acceleratorKeys, describeAccelerator } from "./shortcuts/accelerator";
import { bindingFor, ShortcutSettingsProvider, useShortcutSettings } from "./shortcuts/context";
import { TauriEvent } from "./tauriProtocol";
import { AppUpdater } from "./updater";

const destinations = [
  { label: "New note", to: "/" },
  { label: "Search", to: "/search" },
  { label: "Add", to: "/add" },
  { label: "Library", to: "/library" },
  { label: "Research", to: "/research" },
  { label: "Settings", to: "/settings" },
] as const;

export function App() {
  return (
    <ShortcutSettingsProvider>
      <AppUpdater />
      <AppShell />
    </ShortcutSettingsProvider>
  );
}

function AppShell() {
  const navigate = useNavigate();
  const [searchOpen, setSearchOpen] = useState(false);
  const { settings } = useShortcutSettings();
  const quickAddAccelerator =
    bindingFor(settings?.keyboardBindings ?? [], KoshCommand.QuickAdd)?.accelerator ??
    DEFAULT_QUICK_ADD_ACCELERATOR;
  const openNewNote = useCallback(
    () =>
      navigate({
        to: "/new/$noteId",
        params: { noteId: createUuidV7() },
      }),
    [navigate],
  );

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen(TauriEvent.OpenSettings, () => {
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

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<"BACK" | "FORWARD" | "NEW_NOTE">(TauriEvent.NavigationCommand, (event) => {
      if (!active) return;
      if (event.payload === "NEW_NOTE") void openNewNote();
      else if (event.payload === "BACK") window.history.back();
      else window.history.forward();
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [openNewNote]);

  useEffect(() => {
    if ("__TAURI_INTERNALS__" in window) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.isComposing ||
        event.altKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.key.toLowerCase() !== "n" ||
        !event.metaKey
      ) {
        return;
      }
      event.preventDefault();
      void openNewNote();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openNewNote]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.isComposing ||
        event.altKey ||
        event.ctrlKey ||
        event.shiftKey ||
        !event.metaKey ||
        event.key.toLowerCase() !== "k"
      ) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      setSearchOpen(true);
      window.requestAnimationFrame(() => {
        document.querySelector<HTMLInputElement>("[data-kosh-search-input]")?.focus();
      });
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);

  return (
    <ErrorBoundary>
      <div className="app-shell">
        <aside className="app-sidebar">
          <Link aria-label="Kosh home" className="app-brand" to="/">
            <span aria-hidden="true">
              <img alt="" src="/icon.svg" />
            </span>
            <strong>Kosh</strong>
          </Link>
          <nav aria-label="Primary">
            {destinations.map((destination) => (
              <Link
                activeOptions={{ exact: true }}
                activeProps={{ "aria-current": "page", className: "app-nav-link--active" }}
                className="app-nav-link"
                key={destination.to}
                onClick={(event) => {
                  if (destination.to !== "/search") return;
                  event.preventDefault();
                  setSearchOpen(true);
                }}
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
        <SearchOverlay onClose={() => setSearchOpen(false)} open={searchOpen} />
      </div>
    </ErrorBoundary>
  );
}
