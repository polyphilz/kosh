import {
  createHashHistory,
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from "@tanstack/react-router";
import { App, LandingPage } from "./App";
import { RuntimePage } from "./routes/RuntimePage";

const rootRoute = createRootRoute({ component: App });
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: LandingPage,
});
const runtimeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/runtime",
  component: RuntimePage,
});
const routeTree = rootRoute.addChildren([indexRoute, runtimeRoute]);

export function createAppRouter(history: RouterHistory = createHashHistory()) {
  return createRouter({ history, routeTree });
}

export const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}
