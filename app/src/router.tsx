import {
  createHashHistory,
  redirect,
  createRootRoute,
  createRoute,
  createRouter,
  lazyRouteComponent,
  type RouterHistory,
} from "@tanstack/react-router";
import { App } from "./App";
import { NotePage } from "./routes/NotePage";
import { createUuidV7 } from "./notes/autosave";

const AddPage = lazyRouteComponent(() => import("./routes/AddPage"), "AddPage");
const CatalogPage = lazyRouteComponent(() => import("./routes/CatalogPage"), "CatalogPage");
const LibraryPage = lazyRouteComponent(() => import("./routes/LibraryPage"), "LibraryPage");
const ResearchPage = lazyRouteComponent(() => import("./routes/ResearchPage"), "ResearchPage");
const RuntimePage = lazyRouteComponent(() => import("./routes/RuntimePage"), "RuntimePage");
const SettingsPage = lazyRouteComponent(() => import("./routes/SettingsPage"), "SettingsPage");
const TidbitPage = lazyRouteComponent(() => import("./routes/TidbitPage"), "TidbitPage");

const rootRoute = createRootRoute({ component: App });
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({
      to: "/new/$noteId",
      params: { noteId: createUuidV7() },
      replace: true,
    });
  },
});
const newNoteRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/new/$noteId",
  component: NewNoteRoute,
});
const noteRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/notes/$noteId",
  component: DurableNoteRoute,
  validateSearch: noteSearch,
});
const searchRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/search",
  component: SearchRoute,
});
const addRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/add",
  component: AddPage,
});
const researchRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/research",
  component: ResearchPage,
});
const libraryRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/library",
  component: LibraryPage,
  validateSearch: libraryBrowseSearch,
});
const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});
const catalogRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/catalog",
  component: CatalogPage,
});
const runtimeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/runtime",
  component: RuntimePage,
});
const tidbitRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/tidbits/$tidbitId",
  component: TidbitPage,
  validateSearch: passageSearch,
});
const routeTree = rootRoute.addChildren([
  indexRoute,
  newNoteRoute,
  noteRoute,
  searchRoute,
  addRoute,
  researchRoute,
  libraryRoute,
  settingsRoute,
  catalogRoute,
  runtimeRoute,
  tidbitRoute,
]);

function NewNoteRoute() {
  const { noteId } = newNoteRoute.useParams();
  return <NotePage key={`ephemeral:${noteId}`} mode="ephemeral" noteId={noteId} />;
}

function DurableNoteRoute() {
  const { noteId } = noteRoute.useParams();
  const { passage } = noteRoute.useSearch();
  return <NotePage key={`durable:${noteId}`} mode="durable" noteId={noteId} passageId={passage} />;
}

function SearchRoute() {
  return null;
}

export function createAppRouter(history: RouterHistory = createHashHistory()) {
  return createRouter({ history, routeTree });
}

export const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}

function passageSearch(search: Record<string, unknown>): {
  exact?: true;
  from?: "library" | "research" | "search";
  passage?: string;
  q?: string;
  revision?: string;
  view?: "all" | "recent" | "trash";
} {
  const passage =
    typeof search.passage === "string" && search.passage.length <= 256 ? search.passage : undefined;
  const revision =
    typeof search.revision === "string" && search.revision.length <= 64
      ? search.revision
      : undefined;
  const from =
    search.from === "library" || search.from === "research" || search.from === "search"
      ? search.from
      : undefined;
  const view =
    search.view === "all" || search.view === "recent" || search.view === "trash"
      ? search.view
      : undefined;
  const q =
    typeof search.q === "string" && [...search.q].length <= 512 && search.q.trim()
      ? search.q
      : undefined;
  const exact = search.exact === true || search.exact === "true" ? true : undefined;
  return {
    ...(exact ? { exact } : {}),
    ...(from ? { from } : {}),
    ...(passage ? { passage } : {}),
    ...(q ? { q } : {}),
    ...(revision ? { revision } : {}),
    ...(view ? { view } : {}),
  };
}

function libraryBrowseSearch(search: Record<string, unknown>): {
  view?: "all" | "recent" | "trash";
} {
  const view =
    search.view === "all" || search.view === "trash" || search.view === "recent"
      ? search.view
      : undefined;
  return view && view !== "recent" ? { view } : {};
}

function noteSearch(search: Record<string, unknown>): {
  passage?: string;
} {
  const passage = passageSearch(search).passage;
  return passage ? { passage } : {};
}
