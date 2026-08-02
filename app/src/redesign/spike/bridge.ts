import type { KoshSpikeEditor, KoshSpikePartialBlock } from "./schema";
import { koshBlocksToMarkdown, markdownToKoshBlocks } from "../../editor/markdownAdapter";
import type { BlockNoteMediaController } from "../../editor/mediaController";
import type { SelectedAttachmentRecord } from "../../backend/contracts";

export interface BlockNoteSpikeSnapshot {
  blocks: unknown[];
  focused: boolean;
  selectedBlockIds: string[];
}

export interface BlockNoteSpikeBridge {
  appendParagraph(text?: string): string;
  beginDeferredMedia(): string;
  capability: "blocknote";
  installLongDocument(blockCount: number): void;
  installListPair(): { firstId: string; secondId: string };
  installRetryMediaFixture(kind: BlockNoteSpikeMediaKind): void;
  loadMarkdown(markdown: string): void;
  markdown(): string;
  mediaStatusCalls(kind: BlockNoteSpikeMediaKind): number;
  insertMediaFixture(): void;
  resolveDeferredMedia(requestId: string, outcome: "cancel" | "failure" | "success"): void;
  schema: {
    blocks: readonly string[];
    inlineContent: readonly string[];
    styles: readonly string[];
  };
  selectBlocks(startId: string, endId: string): void;
  snapshot(): BlockNoteSpikeSnapshot;
}

export type BlockNoteSpikeMediaKind = "image" | "pdf";

export interface BlockNoteSpikeMediaHarness {
  prepareRetry(kind: BlockNoteSpikeMediaKind): void;
  statusCalls(kind: BlockNoteSpikeMediaKind): number;
}

declare global {
  interface Window {
    __KOSH_BLOCKNOTE_SPIKE__?: BlockNoteSpikeBridge;
  }
}

export function installSpikeBridge(
  editor: KoshSpikeEditor,
  schema: BlockNoteSpikeBridge["schema"],
  mediaController: BlockNoteMediaController,
  mediaHarness: BlockNoteSpikeMediaHarness,
): () => void {
  const deferredMedia = new Map<
    string,
    {
      reject: (error: Error) => void;
      resolve: (record: SelectedAttachmentRecord | null) => void;
    }
  >();
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
    beginDeferredMedia() {
      const requestId = crypto.randomUUID();
      void mediaController.begin(
        "Adding deferred image",
        () =>
          new Promise((resolve, reject) => {
            deferredMedia.set(requestId, { reject, resolve });
          }),
      );
      return requestId;
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
    installRetryMediaFixture(kind) {
      mediaHarness.prepareRetry(kind);
      mediaController.insert([mediaFixtureRecords()[kind === "image" ? 0 : 1]!]);
    },
    loadMarkdown(markdown) {
      editor.replaceBlocks(editor.document, markdownToKoshBlocks(markdown));
      editor.setTextCursorPosition(editor.document[0]!, "start");
      editor.focus();
    },
    markdown() {
      return koshBlocksToMarkdown(editor.document);
    },
    mediaStatusCalls(kind) {
      return mediaHarness.statusCalls(kind);
    },
    insertMediaFixture() {
      mediaController.insert(mediaFixtureRecords());
    },
    resolveDeferredMedia(requestId, outcome) {
      const deferred = deferredMedia.get(requestId);
      if (!deferred) throw new Error(`Unknown deferred media request ${requestId}`);
      deferredMedia.delete(requestId);
      if (outcome === "failure") deferred.reject(new Error("Synthetic media failure"));
      else deferred.resolve(outcome === "success" ? mediaFixtureRecords()[0]! : null);
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
    for (const deferred of deferredMedia.values()) deferred.resolve(null);
    deferredMedia.clear();
    if (window.__KOSH_BLOCKNOTE_SPIKE__ === bridge) {
      delete window.__KOSH_BLOCKNOTE_SPIKE__;
    }
  };
}

export function mediaFixtureRecords(): SelectedAttachmentRecord[] {
  return [
    {
      recordKind: "IMAGE",
      record: {
        id: "019f547b-6200-7000-8000-000000000101",
        ingestLeaseId: "spike-image-lease",
        displayFilename: "diagram.png",
        mediaType: "image/png",
        byteLength: 1_024,
        kind: "IMAGE",
        naturalWidth: 640,
        naturalHeight: 480,
        ocrStatus: "READY",
        ocrError: null,
      },
    },
    {
      recordKind: "PDF",
      record: {
        id: "019f547b-6200-7000-8000-000000000102",
        ingestLeaseId: "spike-pdf-lease",
        displayFilename: "chapter.pdf",
        mediaType: "application/pdf",
        byteLength: 4_096,
        kind: "PDF",
        pageCount: 12,
        extractionStatus: "READY",
        extractionError: null,
      },
    },
    {
      recordKind: "GENERIC",
      record: {
        id: "019f547b-6200-7000-8000-000000000103",
        ingestLeaseId: "spike-file-lease",
        displayFilename: "appendix.txt",
        mediaType: "text/plain",
        byteLength: 2_048,
        kind: "TEXT",
        extractionStatus: "READY",
        extractionError: null,
        extractedLineCount: 20,
      },
    },
  ];
}

export function isBlockNoteCapability(value: unknown): value is BlockNoteSpikeBridge {
  return (
    typeof value === "object" &&
    value !== null &&
    Reflect.get(value, "capability") === "blocknote" &&
    typeof Reflect.get(value, "snapshot") === "function"
  );
}
