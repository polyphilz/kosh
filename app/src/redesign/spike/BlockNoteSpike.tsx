import {
  filterSuggestionItems,
  insertOrUpdateBlockForSlashMenu,
  SideMenuExtension,
} from "@blocknote/core/extensions";
import { BlockNoteView } from "@blocknote/mantine";
import {
  DragHandleMenu,
  SideMenu,
  SideMenuController,
  type SideMenuProps,
  SuggestionMenuController,
  type DefaultReactSuggestionItem,
  useBlockNoteEditor,
  useComponentsContext,
  useCreateBlockNote,
  useExtension,
  useExtensionState,
} from "@blocknote/react";
import { MantineProvider } from "@mantine/core";
import { useEffect, useMemo } from "react";
import { createBlockNoteMediaController } from "../../editor/mediaController";
import { KoshMediaActionsProvider, type KoshMediaActions } from "../../editor/mediaBlocks";
import { installSpikeBridge, mediaFixtureRecords } from "./bridge";
import {
  initialSpikeBlocks,
  koshSpikeSchema,
  supportedSpikeBlockTypes,
  supportedSpikeInlineTypes,
  supportedSpikeStyleTypes,
  type KoshSpikeEditor,
  type KoshSpikePartialBlock,
} from "./schema";

export interface BlockNoteSpikeProps {
  theme: "light" | "dark";
}

export function BlockNoteSpike({ theme }: BlockNoteSpikeProps) {
  const editor = useCreateBlockNote({
    schema: koshSpikeSchema,
    initialContent: initialSpikeBlocks,
    tabBehavior: "prefer-indent",
  });
  const mediaController = useMemo(() => createBlockNoteMediaController(editor), [editor]);
  const slashItems = useMemo(
    () => restrictedSlashItems(editor, mediaController),
    [editor, mediaController],
  );
  const mediaActions = useMemo<KoshMediaActions>(() => spikeMediaActions(), []);

  useEffect(
    () =>
      installSpikeBridge(
        editor,
        {
          blocks: supportedSpikeBlockTypes,
          inlineContent: supportedSpikeInlineTypes,
          styles: supportedSpikeStyleTypes,
        },
        mediaController,
      ),
    [editor, mediaController],
  );
  useEffect(() => {
    mediaController.activate();
    return () => mediaController.dispose();
  }, [mediaController]);

  return (
    <MantineProvider forceColorScheme={theme}>
      <KoshMediaActionsProvider actions={mediaActions}>
        <main
          className="kosh-blocknote-spike"
          data-theme={theme}
          onDropCapture={(event) => {
            if (![...(event.dataTransfer?.types ?? [])].includes("application/x-kosh-media")) {
              return;
            }
            event.preventDefault();
            mediaController.insert(mediaFixtureRecords());
          }}
          onPasteCapture={(event) =>
            mediaController.handleImagePaste(
              event.nativeEvent,
              async () => mediaFixtureRecords()[0]!,
            )
          }
        >
          <p className="kosh-blocknote-spike__label">Isolated BlockNote feasibility harness</p>
          <BlockNoteView
            comments={false}
            editor={editor}
            emojiPicker={false}
            filePanel={false}
            formattingToolbar
            slashMenu={false}
            sideMenu={false}
            tableHandles={false}
            theme={theme}
          >
            <SuggestionMenuController
              getItems={async (query) => filterSuggestionItems(slashItems, query)}
              triggerCharacter="/"
            />
            <SideMenuController sideMenu={KoshSpikeSideMenu} />
          </BlockNoteView>
        </main>
      </KoshMediaActionsProvider>
    </MantineProvider>
  );
}

function spikeMediaActions(): KoshMediaActions {
  const records = mediaFixtureRecords();
  const image = records[0]!.recordKind === "IMAGE" ? records[0].record : null;
  const pdf = records[1]!.recordKind === "PDF" ? records[1].record : null;
  const file = records[2]!.recordKind === "GENERIC" ? records[2].record : null;
  if (!image || !pdf || !file) throw new Error("Invalid media spike fixtures");
  return {
    attachmentStatus: async (attachmentId) => ({
      attachmentId,
      byteLength: file.byteLength,
      displayFilename: file.displayFilename,
      extractedLineCount: file.extractedLineCount,
      extractionError: file.extractionError,
      extractionStatus: file.extractionStatus,
      kind: file.kind,
      mediaType: file.mediaType,
    }),
    imageStatus: async (attachmentId) => ({
      attachmentId,
      naturalHeight: image.naturalHeight,
      naturalWidth: image.naturalWidth,
      nextAttemptAtMs: null,
      ocrError: image.ocrError,
      ocrStatus: image.ocrStatus,
    }),
    mediaUrl: () =>
      "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='640' height='480'%3E%3Crect width='640' height='480' fill='%23d97745'/%3E%3C/svg%3E",
    openAttachmentExternal: async () => undefined,
    openPdfExternal: async () => undefined,
    pdfStatus: async (attachmentId) => ({
      attachmentId,
      displayFilename: pdf.displayFilename,
      extractedPageCount: pdf.pageCount,
      extractionError: pdf.extractionError,
      extractionStatus: pdf.extractionStatus,
      nextAttemptAtMs: null,
      pageCount: pdf.pageCount,
      unavailablePageCount: 0,
    }),
    pickReplacement: async () => records[0]!,
    revealAttachmentInFinder: async () => undefined,
  };
}

function KoshSpikeDragMenu() {
  return (
    <DragHandleMenu>
      <KoshMoveBlockItem direction="up" />
      <KoshMoveBlockItem direction="down" />
      <KoshRemoveBlockItem />
    </DragHandleMenu>
  );
}

function KoshMoveBlockItem({ direction }: { direction: "down" | "up" }) {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshSpikeSchema);
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;
  return (
    <Components.Generic.Menu.Item
      className="bn-menu-item"
      onClick={() => {
        if (direction === "up") editor.moveBlocksUp(hoveredBlock);
        else editor.moveBlocksDown(hoveredBlock);
        requestAnimationFrame(() => editor.focus());
      }}
    >
      Move block {direction}
    </Components.Generic.Menu.Item>
  );
}

function KoshRemoveBlockItem() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshSpikeSchema);
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;

  return (
    <Components.Generic.Menu.Item
      className="bn-menu-item"
      onClick={() => {
        const selectedBlocks = editor.getSelection()?.blocks;
        const blocksToRemove =
          selectedBlocks?.some((block) => block.id === hoveredBlock.id) === true
            ? selectedBlocks
            : [hoveredBlock];
        const topLevelIndex = Math.max(
          0,
          editor.document.findIndex((block) => block.id === blocksToRemove[0]?.id),
        );
        editor.removeBlocks(blocksToRemove);
        requestAnimationFrame(() => {
          const focusTarget = editor.document[Math.min(topLevelIndex, editor.document.length - 1)];
          if (focusTarget) editor.setTextCursorPosition(focusTarget, "start");
          editor.focus();
        });
      }}
    >
      Delete selected blocks
    </Components.Generic.Menu.Item>
  );
}

function KoshSpikeSideMenu(properties: SideMenuProps) {
  return (
    <SideMenu {...properties}>
      <KoshSpikeDragHandleButton />
    </SideMenu>
  );
}

function KoshSpikeDragHandleButton() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshSpikeSchema);
  const sideMenu = useExtension(SideMenuExtension, { editor });
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;

  return (
    <Components.Generic.Menu.Root
      onOpenChange={(open) => {
        if (open) sideMenu.freezeMenu();
        else sideMenu.unfreezeMenu();
      }}
      position="left"
    >
      <Components.Generic.Menu.Trigger>
        <Components.SideMenu.Button
          className="bn-button"
          draggable
          icon={<span aria-hidden>⋮⋮</span>}
          label="Open block menu"
          onDragEnd={() => {
            sideMenu.blockDragEnd();
            requestAnimationFrame(() => {
              const focusTarget = editor.getBlock(hoveredBlock.id) ?? editor.document[0];
              if (focusTarget) editor.setTextCursorPosition(focusTarget, "start");
              editor.focus();
            });
          }}
          onDragStart={(event) => sideMenu.blockDragStart(event, hoveredBlock)}
        />
      </Components.Generic.Menu.Trigger>
      <KoshSpikeDragMenu />
    </Components.Generic.Menu.Root>
  );
}

function restrictedSlashItems(
  editor: KoshSpikeEditor,
  mediaController: ReturnType<typeof createBlockNoteMediaController>,
): DefaultReactSuggestionItem[] {
  return [
    blockItem(editor, "Paragraph", { type: "paragraph" }, ["text", "body"]),
    blockItem(editor, "Heading 1", { type: "heading", props: { level: 1 } }, ["h1"]),
    blockItem(editor, "Heading 2", { type: "heading", props: { level: 2 } }, ["h2"]),
    blockItem(editor, "Heading 3", { type: "heading", props: { level: 3 } }, ["h3"]),
    blockItem(editor, "Bullet list", { type: "bulletListItem" }, ["unordered", "ul"]),
    blockItem(editor, "Ordered list", { type: "numberedListItem" }, ["numbered", "ol"]),
    blockItem(editor, "Code block", { type: "codeBlock", props: { language: "" } }, ["fence"]),
    blockItem(editor, "Display math", { type: "displayMath", props: { latex: "\\sum_i a_i" } }, [
      "equation",
      "math",
    ]),
    {
      title: "Inline math",
      aliases: ["equation", "math"],
      group: "Kosh blocks",
      onItemClick: () => {
        insertOrUpdateBlockForSlashMenu(editor, { type: "paragraph" });
        editor.insertInlineContent([{ type: "inlineMath", props: { latex: "a_i" } }], {
          updateSelection: true,
        });
      },
    },
    mediaItem(editor, mediaController, "Image", "image", 0),
    mediaItem(editor, mediaController, "PDF", "document", 1),
    mediaItem(editor, mediaController, "File", "attachment", 2),
  ];
}

function mediaItem(
  editor: KoshSpikeEditor,
  controller: ReturnType<typeof createBlockNoteMediaController>,
  title: string,
  alias: string,
  fixtureIndex: number,
): DefaultReactSuggestionItem {
  return {
    title,
    aliases: [alias, "upload"],
    group: "Kosh media",
    onItemClick: () => {
      insertOrUpdateBlockForSlashMenu(editor, { type: "paragraph" });
      void controller.begin(`Adding ${title.toLowerCase()}`, async () => {
        const record = mediaFixtureRecords()[fixtureIndex];
        if (!record) throw new Error(`Missing ${title} media fixture`);
        return record;
      });
    },
  };
}

function blockItem(
  editor: KoshSpikeEditor,
  title: string,
  block: KoshSpikePartialBlock,
  aliases: string[],
): DefaultReactSuggestionItem {
  return {
    title,
    aliases,
    group: "Kosh blocks",
    onItemClick: () => insertOrUpdateBlockForSlashMenu(editor, block),
  };
}
