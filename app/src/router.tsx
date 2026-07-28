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
import { RuntimePage } from "./routes/RuntimePage";
import { SearchPage } from "./routes/SearchPage";
import { SettingsPage } from "./routes/SettingsPage";
import { TidbitPage } from "./routes/TidbitPage";

const rootRoute = createRootRoute({ component: App });
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: SearchPage,
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
});
const routeTree = rootRoute.addChildren([
  indexRoute,
  addRoute,
  researchRoute,
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
