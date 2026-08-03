import "@mantine/core/styles.css";
import "@blocknote/mantine/style.css";
import "./spike.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BlockNoteSpike } from "./BlockNoteSpike";

const root = document.querySelector("#blocknote-spike-root");
if (!root) throw new Error("BlockNote spike root is missing");
const parameters = new URLSearchParams(window.location.search);
const theme = parameters.get("theme") === "dark" ? "dark" : "light";
const plain = parameters.get("mode") === "plain";

createRoot(root).render(
  <StrictMode>
    {plain ? (
      <main className="kosh-blocknote-spike" data-theme={theme}>
        <p className="kosh-blocknote-spike__label">Plain contenteditable control</p>
        <div
          aria-label="Plain editor"
          contentEditable
          role="textbox"
          suppressContentEditableWarning
        >
          A plain contenteditable cannot satisfy the Kosh block contract.
        </div>
      </main>
    ) : (
      <BlockNoteSpike theme={theme} />
    )}
  </StrictMode>,
);
