import "@mantine/core/styles.css";
import "@blocknote/mantine/style.css";
import "@fontsource-variable/reddit-mono";
import "katex/dist/katex.min.css";
import "../../typography.css";
import "../editor.css";
import "./harness.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BlockNoteHarness } from "./BlockNoteHarness";

const root = document.querySelector("#editor-harness-root");
if (!root) throw new Error("BlockNote harness root is missing");
const parameters = new URLSearchParams(window.location.search);
const theme = parameters.get("theme") === "dark" ? "dark" : "light";

createRoot(root).render(
  <StrictMode>
    <BlockNoteHarness theme={theme} />
  </StrictMode>,
);
