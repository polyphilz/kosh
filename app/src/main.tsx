import { RouterProvider } from "@tanstack/react-router";
import React from "react";
import ReactDOM from "react-dom/client";
import { BackendProvider } from "./backend/context";
import { createBackend } from "./backend/createBackend";
import { router } from "./router";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BackendProvider backend={createBackend()}>
      <RouterProvider router={router} />
    </BackendProvider>
  </React.StrictMode>,
);
