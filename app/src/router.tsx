import {
  createHashHistory,
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from "@tanstack/react-router";
import { App } from "./App";
import { AddPage } from "./routes/AddPage";
import { CatalogPage } from "./routes/CatalogPage";
import { ResearchPage } from "./routes/ResearchPage";
import { LibraryPage } from "./routes/LibraryPage";
import { RuntimePage } from "./routes/RuntimePage";
import { SearchPage } from "./routes/SearchPage";
import { SettingsPage } from "./routes/SettingsPage";
import { TidbitPage } from "./routes/TidbitPage";

const rootRoute = createRootRoute({ component: App });
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: SearchPage,
  validateSearch: librarySearch,
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
  addRoute,
  researchRoute,
  libraryRoute,
  settingsRoute,
  catalogRoute,
  runtimeRoute,
  tidbitRoute,
]);

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

function librarySearch(search: Record<string, unknown>): {
  exact?: true;
  passage?: string;
  q?: string;
} {
  const passage = passageSearch(search).passage;
  const q =
    typeof search.q === "string" && [...search.q].length <= 512 && search.q.trim()
      ? search.q
      : undefined;
  const exact = search.exact === true || search.exact === "true" ? true : undefined;
  return {
    ...(q ? { q } : {}),
    ...(exact ? { exact } : {}),
    ...(passage ? { passage } : {}),
  };
}
