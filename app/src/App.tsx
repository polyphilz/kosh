import { Link, Outlet, useNavigate, useRouterState } from "@tanstack/react-router";
import { listen } from "@tauri-apps/api/event";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { useCallback, useEffect, useRef, useState } from "react";
import type { TidbitRecord } from "./backend/contracts";
import { useBackend } from "./backend/context";
import { ErrorBoundary } from "./components/States";
import { clearFindInNoteRequest, requestFindInNote } from "./editor/findInNote";
import { createUuidV7 } from "./notes/autosave";
import { NoteDeletionContext } from "./notes/deletion";
import { SearchOverlay } from "./search/SearchOverlay";
import { checkpointBeforeSearch } from "./search/checkpoint";
import { ShortcutSettingsProvider, useShortcutSettings } from "./shortcuts/context";
import {
  LocalShortcutCommand,
  keyboardEventMatchesAccelerator,
  localBindingFor,
  noteLinkForLocation,
  noteTargetForDeepLink,
} from "./shortcuts/localShortcuts";
import { TauriEvent } from "./tauriProtocol";
import { AppUpdater } from "./updater";

const NOTE_UNDO_DURATION_MS = 10_000;
const LINK_COPY_NOTICE_DURATION_MS = 1_800;
const SIDEBAR_OPEN_STORAGE_KEY = "kosh.sidebar.open.v1";

export function App() {
  return (
    <ShortcutSettingsProvider>
      <AppUpdater />
      <AppShell />
    </ShortcutSettingsProvider>
  );
}

function AppShell() {
  const backend = useBackend();
  const { activeLocalBindings } = useShortcutSettings();
  const navigate = useNavigate();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const [searchOpen, setSearchOpen] = useState(false);
  const searchRouteOpen = pathname === "/search";
  const noteRouteOpen = /^\/(?:new|notes)\//u.test(pathname);
  const [sidebarOpen, setSidebarOpen] = useState(readSidebarOpen);
  const [deletedNote, setDeletedNote] = useState<TidbitRecord | null>(null);
  const [undoing, setUndoing] = useState(false);
  const [undoError, setUndoError] = useState<string | null>(null);
  const [linkCopyNotice, setLinkCopyNotice] = useState<{
    message: string;
    tone: "danger" | "success";
  } | null>(null);
  const undoTimer = useRef<number | null>(null);
  const linkCopyTimer = useRef<number | null>(null);
  const openNewNote = useCallback(
    () =>
      navigate({
        to: "/new/$noteId",
        params: { noteId: createUuidV7() },
      }),
    [navigate],
  );
  const openSearch = useCallback(() => {
    setSearchOpen(true);
    window.requestAnimationFrame(() => {
      document.querySelector<HTMLInputElement>("[data-kosh-search-input]")?.focus();
    });
  }, []);
  const toggleSidebar = useCallback(() => setSidebarOpen((current) => !current), []);
  const clearUndoTimer = useCallback(() => {
    if (undoTimer.current === null) return;
    window.clearTimeout(undoTimer.current);
    undoTimer.current = null;
  }, []);
  const announceDeletedNote = useCallback(
    (note: TidbitRecord) => {
      clearUndoTimer();
      setDeletedNote(note);
      setUndoError(null);
      setUndoing(false);
      undoTimer.current = window.setTimeout(() => {
        undoTimer.current = null;
        setDeletedNote(null);
        setUndoError(null);
      }, NOTE_UNDO_DURATION_MS);
    },
    [clearUndoTimer],
  );

  useEffect(() => () => clearUndoTimer(), [clearUndoTimer]);

  useEffect(() => () => clearFindInNoteRequest(pathname), [pathname]);

  useEffect(
    () => () => {
      if (linkCopyTimer.current !== null) window.clearTimeout(linkCopyTimer.current);
    },
    [],
  );

  useEffect(() => {
    try {
      window.localStorage.setItem(SIDEBAR_OPEN_STORAGE_KEY, sidebarOpen ? "true" : "false");
    } catch {
      // Sidebar persistence is optional; navigation must remain available.
    }
  }, [sidebarOpen]);

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
    const openDeepLink = (urls: string[], replace: boolean) => {
      if (!active) return;
      const target = urls.map(noteTargetForDeepLink).find((candidate) => candidate !== null);
      if (!target) return;
      setSearchOpen(false);
      void navigate({
        to: "/notes/$noteId",
        params: { noteId: target.noteId },
        search: {
          ...(target.passage ? { passage: target.passage } : {}),
          ...(target.query ? { query: target.query } : {}),
        },
        replace,
      });
    };
    void getCurrent()
      .then((urls) => {
        if (urls) openDeepLink(urls, true);
      })
      .catch((reason: unknown) => console.error("Could not read the launch link", reason));
    void onOpenUrl((urls) => openDeepLink(urls, false))
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch((reason: unknown) => console.error("Could not listen for note links", reason));
    return () => {
      active = false;
      unlisten?.();
    };
  }, [navigate]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<"BACK" | "FORWARD" | "NEW_NOTE" | "SEARCH" | "TOGGLE_SIDEBAR">(
      TauriEvent.NavigationCommand,
      (event) => {
        if (!active) return;
        if (event.payload === "NEW_NOTE") void openNewNote();
        else if (event.payload === "SEARCH") openSearch();
        else if (event.payload === "TOGGLE_SIDEBAR") toggleSidebar();
        else if (event.payload === "BACK") window.history.back();
        else window.history.forward();
      },
    ).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [openNewNote, openSearch, toggleSidebar]);

  useEffect(() => {
    const copyNoteLink = localBindingFor(activeLocalBindings, LocalShortcutCommand.CopyNoteLink);
    const copyExactNoteLink = localBindingFor(
      activeLocalBindings,
      LocalShortcutCommand.CopyExactNoteLink,
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (!noteRouteOpen || event.isComposing || event.repeat) return;
      const exact =
        copyExactNoteLink !== undefined &&
        keyboardEventMatchesAccelerator(event, copyExactNoteLink.accelerator);
      const clean =
        copyNoteLink !== undefined &&
        keyboardEventMatchesAccelerator(event, copyNoteLink.accelerator);
      if (!exact && !clean) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      void checkpointBeforeSearch()
        .then(async () => {
          const link = noteLinkForLocation(window.location.href, exact);
          const target = noteTargetForDeepLink(link);
          if (!target) throw new Error("The current page is not a linkable note.");
          await backend.loadTidbit(target.noteId);
          await backend.copyText(link);
        })
        .then(
          () => {
            if (linkCopyTimer.current !== null) window.clearTimeout(linkCopyTimer.current);
            setLinkCopyNotice({
              message: exact ? "Exact note link copied" : "Note link copied",
              tone: "success",
            });
            linkCopyTimer.current = window.setTimeout(() => {
              linkCopyTimer.current = null;
              setLinkCopyNotice(null);
            }, LINK_COPY_NOTICE_DURATION_MS);
          },
          (reason: unknown) => {
            if (linkCopyTimer.current !== null) window.clearTimeout(linkCopyTimer.current);
            setLinkCopyNotice({
              message: `Could not copy note link: ${errorMessage(reason)}`,
              tone: "danger",
            });
            linkCopyTimer.current = window.setTimeout(() => {
              linkCopyTimer.current = null;
              setLinkCopyNotice(null);
            }, LINK_COPY_NOTICE_DURATION_MS);
          },
        );
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [activeLocalBindings, backend, noteRouteOpen]);

  useEffect(() => {
    let pendingFrame: number | null = null;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        !noteRouteOpen ||
        event.isComposing ||
        event.altKey ||
        event.ctrlKey ||
        event.shiftKey ||
        !event.metaKey ||
        event.key.toLowerCase() !== "f" ||
        document.querySelector('[aria-modal="true"]')
      ) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      setSearchOpen(false);
      if (pendingFrame !== null) window.cancelAnimationFrame(pendingFrame);
      pendingFrame = window.requestAnimationFrame(() => {
        pendingFrame = null;
        requestFindInNote(pathname);
      });
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      if (pendingFrame !== null) window.cancelAnimationFrame(pendingFrame);
    };
  }, [noteRouteOpen, pathname]);

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
      openSearch();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [openSearch]);

  useEffect(() => {
    if ("__TAURI_INTERNALS__" in window) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.isComposing ||
        event.altKey ||
        event.ctrlKey ||
        event.shiftKey ||
        !event.metaKey ||
        (event.key !== "/" && event.code !== "Slash")
      ) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      toggleSidebar();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [toggleSidebar]);

  const undoDelete = useCallback(async () => {
    if (!deletedNote || undoing) return;
    clearUndoTimer();
    setUndoing(true);
    setUndoError(null);
    try {
      const restored = await backend.restoreTidbit({
        id: deletedNote.id,
        expectedRevisionId: deletedNote.currentRevisionId,
      });
      clearUndoTimer();
      setDeletedNote(null);
      await navigate({
        to: "/notes/$noteId",
        params: { noteId: restored.id },
        search: {},
      });
    } catch (reason) {
      setUndoError(errorMessage(reason));
    } finally {
      setUndoing(false);
    }
  }, [backend, clearUndoTimer, deletedNote, navigate, undoing]);

  return (
    <ErrorBoundary>
      <NoteDeletionContext.Provider value={announceDeletedNote}>
        <div className="app-shell" data-sidebar={sidebarOpen ? "open" : "closed"}>
          <button
            aria-controls="kosh-sidebar"
            aria-expanded={sidebarOpen}
            aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
            className="app-sidebar-toggle"
            onClick={toggleSidebar}
            title={`${sidebarOpen ? "Hide" : "Show"} sidebar (⌘/)`}
            type="button"
          >
            <span aria-hidden="true">{sidebarOpen ? "‹" : "›"}</span>
          </button>
          <aside className="app-sidebar" hidden={!sidebarOpen} id="kosh-sidebar">
            <div className="app-brand">
              <span aria-hidden="true">
                <img alt="" src="/icon.svg" />
              </span>
              <strong>Kosh</strong>
            </div>
            <nav aria-label="Primary">
              <button
                className="app-nav-link"
                onClick={() => void openNewNote()}
                title="New note (⌘N)"
                type="button"
              >
                <span aria-hidden="true">＋</span>
                New note
              </button>
              <button
                className="app-nav-link"
                onClick={openSearch}
                title="Search notes (⌘K)"
                type="button"
              >
                <span aria-hidden="true">⌕</span>
                Search
              </button>
              <Link
                activeOptions={{ exact: true }}
                activeProps={{ "aria-current": "page", className: "app-nav-link--active" }}
                className="app-nav-link"
                title="Settings (⌘,)"
                to="/settings"
              >
                <span aria-hidden="true">⚙</span>
                Settings
              </Link>
            </nav>
          </aside>
          <div className="app-content">
            <Outlet />
          </div>
          <SearchOverlay
            onClose={() => {
              setSearchOpen(false);
              if (searchRouteOpen) void navigate({ to: "/", replace: true });
            }}
            onResultOpen={() => setSearchOpen(false)}
            open={searchOpen || searchRouteOpen}
          />
          {deletedNote && (
            <div className="note-undo" role={undoError ? "alert" : "status"}>
              <span>
                {undoError
                  ? `Could not restore note: ${undoError}`
                  : `Deleted “${deletedNote.displayTitle}”`}
              </span>
              <button disabled={undoing} onClick={() => void undoDelete()} type="button">
                {undoing ? "Restoring…" : "Undo"}
              </button>
              <button
                aria-label="Dismiss deleted note notice"
                onClick={() => {
                  clearUndoTimer();
                  setDeletedNote(null);
                  setUndoError(null);
                }}
                type="button"
              >
                ×
              </button>
            </div>
          )}
          {linkCopyNotice && (
            <div
              className="link-copy-notice"
              data-tone={linkCopyNotice.tone}
              role={linkCopyNotice.tone === "danger" ? "alert" : "status"}
            >
              {linkCopyNotice.message}
            </div>
          )}
        </div>
      </NoteDeletionContext.Provider>
    </ErrorBoundary>
  );
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

function readSidebarOpen(): boolean {
  try {
    return window.localStorage.getItem(SIDEBAR_OPEN_STORAGE_KEY) !== "false";
  } catch {
    return true;
  }
}
