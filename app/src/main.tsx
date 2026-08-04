import { RouterProvider } from "@tanstack/react-router";
import "@mantine/core/styles.css";
import "@blocknote/mantine/style.css";
import "@fontsource-variable/reddit-mono";
import React from "react";
import ReactDOM from "react-dom/client";
import { BackendProvider } from "./backend/context";
import { createBackend } from "./backend/createBackend";
import { AppearanceProvider } from "./components/Appearance";
import { StartupSmokeReady } from "./components/StartupSmokeReady";
import { QuitCoordinator } from "./lifecycle/quit";
import { router } from "./router";
import "./components/components.css";
import "katex/dist/katex.min.css";
import "./typography.css";
import "./editor/editor.css";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BackendProvider backend={createBackend()}>
      <AppearanceProvider>
        <StartupSmokeReady surface="main" />
        <QuitCoordinator />
        <RouterProvider router={router} />
      </AppearanceProvider>
    </BackendProvider>
  </React.StrictMode>,
);
