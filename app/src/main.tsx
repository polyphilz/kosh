import { RouterProvider } from "@tanstack/react-router";
import "@fontsource-variable/reddit-mono";
import React from "react";
import ReactDOM from "react-dom/client";
import { BackendProvider } from "./backend/context";
import { createBackend } from "./backend/createBackend";
import { AppearanceProvider } from "./components/Appearance";
import { router } from "./router";
import "./components/components.css";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BackendProvider backend={createBackend()}>
      <AppearanceProvider>
        <RouterProvider router={router} />
      </AppearanceProvider>
    </BackendProvider>
  </React.StrictMode>,
);
