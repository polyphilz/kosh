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
import { useEffect, useMemo, type PointerEvent as ReactPointerEvent } from "react";
import { createBlockNoteMediaController } from "../mediaController";
import { KoshMediaActionsProvider, type KoshMediaActions } from "../mediaBlocks";
import {
  installHarnessBridge,
  mediaFixtureRecords,
  type BlockNoteHarnessMediaHarness,
  type BlockNoteHarnessMediaKind,
} from "./bridge";
import {
  initialHarnessBlocks,
  koshHarnessSchema,
  supportedHarnessBlockTypes,
  supportedHarnessInlineTypes,
  supportedHarnessStyleTypes,
  type KoshHarnessEditor,
  type KoshHarnessPartialBlock,
} from "./schema";

export interface BlockNoteHarnessProps {
  theme: "light" | "dark";
}

export function BlockNoteHarness({ theme }: BlockNoteHarnessProps) {
  const editor = useCreateBlockNote({
    schema: koshHarnessSchema,
    initialContent: initialHarnessBlocks,
    tabBehavior: "prefer-indent",
  });
  const mediaController = useMemo(() => createBlockNoteMediaController(editor), [editor]);
  const slashItems = useMemo(
    () => restrictedSlashItems(editor, mediaController),
    [editor, mediaController],
  );
  const mediaHarness = useMemo(() => createHarnessMediaHarness(), []);
  const sideMenuFloatingUIOptions = useMemo(
    () => ({
      elementProps: {
        onPointerEnter: () => editor.getExtension(SideMenuExtension)?.freezeMenu(),
        onPointerLeave: (event: ReactPointerEvent<HTMLDivElement>) => {
          if (
            event.relatedTarget instanceof Element &&
            event.relatedTarget.closest('[role="menu"]')
          ) {
            return;
          }
          editor.getExtension(SideMenuExtension)?.unfreezeMenu();
        },
      },
    }),
    [editor],
  );

  useEffect(
    () =>
      installHarnessBridge(
        editor,
        {
          blocks: supportedHarnessBlockTypes,
          inlineContent: supportedHarnessInlineTypes,
          styles: supportedHarnessStyleTypes,
        },
        mediaController,
        mediaHarness,
      ),
    [editor, mediaController, mediaHarness],
  );
  useEffect(() => {
    mediaController.activate();
    return () => mediaController.dispose();
  }, [mediaController]);

  return (
    <MantineProvider forceColorScheme={theme}>
      <KoshMediaActionsProvider actions={mediaHarness.actions}>
        <main
          className="kosh-blocknote-editor kosh-editor-harness"
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
          <p className="kosh-editor-harness__label">Isolated BlockNote editor harness</p>
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
            <SideMenuController
              floatingUIOptions={sideMenuFloatingUIOptions}
              sideMenu={KoshHarnessSideMenu}
            />
          </BlockNoteView>
        </main>
      </KoshMediaActionsProvider>
    </MantineProvider>
  );
}

interface HarnessMediaHarness extends BlockNoteHarnessMediaHarness {
  actions: KoshMediaActions;
}

function createHarnessMediaHarness(): HarnessMediaHarness {
  const records = mediaFixtureRecords();
  const image = records[0]!.recordKind === "IMAGE" ? records[0].record : null;
  const pdf = records[1]!.recordKind === "PDF" ? records[1].record : null;
  const file = records[2]!.recordKind === "GENERIC" ? records[2].record : null;
  if (!image || !pdf || !file) throw new Error("Invalid editor harness media fixtures");
  const phases: Record<BlockNoteHarnessMediaKind, "FAILED" | "PENDING" | "READY"> = {
    image: "READY",
    pdf: "READY",
  };
  const statusCalls: Record<BlockNoteHarnessMediaKind, number> = { image: 0, pdf: 0 };
  const actions: KoshMediaActions = {
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
    imageStatus: async (attachmentId) => {
      statusCalls.image += 1;
      const phase = phases.image;
      if (phase === "PENDING") phases.image = "READY";
      return {
        attachmentId,
        naturalHeight: image.naturalHeight,
        naturalWidth: image.naturalWidth,
        nextAttemptAtMs: null,
        ocrError: phase === "FAILED" ? "Synthetic OCR failure" : null,
        ocrStatus: phase === "PENDING" ? "READY" : phase,
      };
    },
    mediaUrl: () =>
      "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='640' height='480'%3E%3Crect width='640' height='480' fill='%23d97745'/%3E%3C/svg%3E",
    openAttachmentExternal: async () => undefined,
    openPdfExternal: async () => undefined,
    pdfStatus: async (attachmentId) => {
      statusCalls.pdf += 1;
      const phase = phases.pdf;
      if (phase === "PENDING") phases.pdf = "READY";
      return {
        attachmentId,
        displayFilename: pdf.displayFilename,
        extractedPageCount: phase === "FAILED" ? 0 : pdf.pageCount,
        extractionError: phase === "FAILED" ? "Synthetic PDF failure" : null,
        extractionStatus: phase === "PENDING" ? "READY" : phase,
        nextAttemptAtMs: null,
        pageCount: pdf.pageCount,
        unavailablePageCount: 0,
      };
    },
    revealAttachmentInFinder: async () => undefined,
    retryImageOcr: async (attachmentId) => {
      phases.image = "PENDING";
      return {
        attachmentId,
        naturalHeight: image.naturalHeight,
        naturalWidth: image.naturalWidth,
        nextAttemptAtMs: null,
        ocrError: null,
        ocrStatus: "PENDING",
      };
    },
    retryPdfExtraction: async (attachmentId) => {
      phases.pdf = "PENDING";
      return {
        attachmentId,
        displayFilename: pdf.displayFilename,
        extractedPageCount: 0,
        extractionError: null,
        extractionStatus: "PENDING",
        nextAttemptAtMs: null,
        pageCount: pdf.pageCount,
        unavailablePageCount: 0,
      };
    },
  };
  return {
    actions,
    prepareRetry(kind) {
      phases[kind] = "FAILED";
      statusCalls[kind] = 0;
    },
    statusCalls(kind) {
      return statusCalls[kind];
    },
  };
}

function KoshHarnessDragMenu() {
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
  const editor = useBlockNoteEditor(koshHarnessSchema);
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
  const editor = useBlockNoteEditor(koshHarnessSchema);
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

function KoshHarnessSideMenu(properties: SideMenuProps) {
  return (
    <SideMenu {...properties}>
      <KoshHarnessAddBlockButton />
      <KoshHarnessDragHandleButton />
    </SideMenu>
  );
}

function KoshHarnessAddBlockButton() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshHarnessSchema);
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;

  return (
    <Components.SideMenu.Button
      className="bn-button kosh-gutter-button kosh-gutter-button--add"
      icon={<span aria-hidden>+</span>}
      label="Click to add below"
      onClick={() => {
        const inserted = editor.insertBlocks([{ type: "paragraph" }], hoveredBlock, "after")[0];
        if (!inserted) return;
        editor.setTextCursorPosition(inserted, "start");
        editor.focus();
      }}
    />
  );
}

function KoshHarnessDragHandleButton() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshHarnessSchema);
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
          className="bn-button kosh-gutter-button kosh-gutter-button--drag"
          draggable
          icon={
            <span aria-hidden className="kosh-gutter-dots">
              {Array.from({ length: 6 }, (_, index) => (
                <i key={index} />
              ))}
            </span>
          }
          label="Drag to move"
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
      <KoshHarnessDragMenu />
    </Components.Generic.Menu.Root>
  );
}

function restrictedSlashItems(
  editor: KoshHarnessEditor,
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
  editor: KoshHarnessEditor,
  controller: ReturnType<typeof createBlockNoteMediaController>,
  title: string,
  alias: string,
  fixtureIndex: number,
): DefaultReactSuggestionItem {
  return {
    title,
    aliases: [alias, "upload"],
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
  editor: KoshHarnessEditor,
  title: string,
  block: KoshHarnessPartialBlock,
  aliases: string[],
): DefaultReactSuggestionItem {
  return {
    title,
    aliases,
    onItemClick: () => insertOrUpdateBlockForSlashMenu(editor, block),
  };
}
