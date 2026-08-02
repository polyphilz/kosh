import type { KoshSpikeEditor, KoshSpikePartialBlock } from "./schema";
import { koshBlocksToMarkdown, markdownToKoshBlocks } from "../../editor/markdownAdapter";

export interface BlockNoteSpikeSnapshot {
  blocks: unknown[];
  focused: boolean;
  selectedBlockIds: string[];
}

export interface BlockNoteSpikeBridge {
  appendParagraph(text?: string): string;
  capability: "blocknote";
  installLongDocument(blockCount: number): void;
  installListPair(): { firstId: string; secondId: string };
  loadMarkdown(markdown: string): void;
  markdown(): string;
  schema: {
    blocks: readonly string[];
    inlineContent: readonly string[];
    styles: readonly string[];
  };
  selectBlocks(startId: string, endId: string): void;
  snapshot(): BlockNoteSpikeSnapshot;
}

declare global {
  interface Window {
    __KOSH_BLOCKNOTE_SPIKE__?: BlockNoteSpikeBridge;
  }
}

export function installSpikeBridge(
  editor: KoshSpikeEditor,
  schema: BlockNoteSpikeBridge["schema"],
): () => void {
  const bridge: BlockNoteSpikeBridge = {
    capability: "blocknote",
    schema,
    appendParagraph(text = "") {
      const reference = editor.document.at(-1);
      if (!reference) throw new Error("editor document is empty");
      const [inserted] = editor.insertBlocks(
        [{ type: "paragraph", content: text }],
        reference,
        "after",
      );
      editor.setTextCursorPosition(inserted, "end");
      editor.focus();
      return inserted.id;
    },
    installLongDocument(blockCount) {
      if (!Number.isSafeInteger(blockCount) || blockCount < 1 || blockCount > 1_000) {
        throw new Error("blockCount must be between 1 and 1000");
      }
      const blocks: KoshSpikePartialBlock[] = Array.from(
        { length: blockCount },
        (_, index): KoshSpikePartialBlock =>
          index % 11 === 0
            ? {
                type: "heading",
                props: { level: 2 },
                content: `Long document block ${index + 1}: ${"bounded editor input ".repeat(8)}`,
              }
            : {
                type: "paragraph",
                content: `Long document block ${index + 1}: ${"bounded editor input ".repeat(8)}`,
              },
      );
      editor.replaceBlocks(editor.document, blocks);
      editor.setTextCursorPosition(editor.document.at(-1)!, "end");
      editor.focus();
    },
    installListPair() {
      const { insertedBlocks } = editor.replaceBlocks(editor.document, [
        { type: "bulletListItem", content: "First item" },
        { type: "bulletListItem", content: "Second item" },
      ]);
      const [first, second] = insertedBlocks;
      editor.setTextCursorPosition(second, "end");
      editor.focus();
      return { firstId: first.id, secondId: second.id };
    },
    loadMarkdown(markdown) {
      editor.replaceBlocks(editor.document, markdownToKoshBlocks(markdown));
      editor.setTextCursorPosition(editor.document[0]!, "start");
      editor.focus();
    },
    markdown() {
      return koshBlocksToMarkdown(editor.document);
    },
    selectBlocks(startId, endId) {
      editor.setSelection(startId, endId);
      editor.focus();
    },
    snapshot() {
      return {
        blocks: structuredClone(editor.document),
        focused: editor.domElement?.contains(document.activeElement) ?? false,
        selectedBlockIds: editor.getSelection()?.blocks.map((block) => block.id) ?? [],
      };
    },
  };
  window.__KOSH_BLOCKNOTE_SPIKE__ = bridge;
  return () => {
    if (window.__KOSH_BLOCKNOTE_SPIKE__ === bridge) {
      delete window.__KOSH_BLOCKNOTE_SPIKE__;
    }
  };
}

export function isBlockNoteCapability(value: unknown): value is BlockNoteSpikeBridge {
  return (
    typeof value === "object" &&
    value !== null &&
    Reflect.get(value, "capability") === "blocknote" &&
    typeof Reflect.get(value, "snapshot") === "function"
  );
}
