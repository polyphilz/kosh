import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import "@fontsource-variable/reddit-mono";
import "katex/dist/katex.min.css";
import React from "react";
import ReactDOM from "react-dom/client";
import { BackendProvider } from "./backend/context";
import { createBackend } from "./backend/createBackend";
import { AppearanceProvider } from "./components/Appearance";
import "./components/components.css";
import "./markdown/markdown.css";
import { QuickAddWindow } from "./quickAdd/QuickAddWindow";
import "./styles.css";
import "./quickAdd/quick-add.css";

const quickAddRoute = createRootRoute({
  component: QuickAddWindow,
});
const quickAddRouter = createRouter({
  history: createMemoryHistory(),
  routeTree: quickAddRoute,
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BackendProvider backend={createBackend()}>
      <AppearanceProvider>
        <RouterProvider router={quickAddRouter} />
      </AppearanceProvider>
    </BackendProvider>
  </React.StrictMode>,
);
