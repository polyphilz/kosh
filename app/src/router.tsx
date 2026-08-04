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

const SettingsPage = lazyRouteComponent(() => import("./routes/SettingsPage"), "SettingsPage");

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
const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});
const routeTree = rootRoute.addChildren([
  indexRoute,
  newNoteRoute,
  noteRoute,
  searchRoute,
  settingsRoute,
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

function noteSearch(search: Record<string, unknown>): { passage?: string; query?: string } {
  const passage =
    typeof search.passage === "string" && [...search.passage].length <= 256
      ? search.passage
      : undefined;
  const query =
    typeof search.query === "string" && [...search.query].length <= 512 ? search.query : undefined;
  return {
    ...(passage ? { passage } : {}),
    ...(query ? { query } : {}),
  };
}
