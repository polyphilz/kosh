import {
  filterSuggestionItems,
  HistoryExtension,
  insertOrUpdateBlockForSlashMenu,
  SideMenuExtension,
} from "@blocknote/core/extensions";
import { BlockNoteView } from "@blocknote/mantine";
import {
  DragHandleMenu,
  SideMenu,
  SideMenuController,
  SuggestionMenuController,
  type DefaultReactSuggestionItem,
  type SideMenuProps,
  useBlockNoteEditor,
  useComponentsContext,
  useCreateBlockNote,
  useExtension,
  useExtensionState,
} from "@blocknote/react";
import { MantineProvider } from "@mantine/core";
import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import type {
  GenericAttachmentStatusRecord,
  ImageRecord,
  ImageStatusRecord,
  PdfRecord,
  PdfStatusRecord,
  SelectedAttachmentRecord,
} from "../backend/contracts";
import { useAppearance } from "../components/Appearance";
import { KoshEditorInteractionProvider } from "./interactionState";
import { koshBlocksToMarkdown, markdownToKoshBlocks } from "./markdownAdapter";
import { KoshMediaActionsProvider, type KoshMediaActions } from "./mediaBlocks";
import { createBlockNoteMediaController } from "./mediaController";
import {
  koshBlockNoteSchema,
  type KoshBlockNoteEditor as KoshBlockNoteEditorInstance,
  type KoshBlockNotePartialBlock,
} from "./schema";

export interface KoshBlockNoteEditorHandle {
  focus: () => void;
  insertAttachments: (attachments: SelectedAttachmentRecord[]) => void;
  insertImages: (images: ImageRecord[]) => void;
  insertPdfs: (pdfs: PdfRecord[]) => void;
}

export interface KoshBlockNoteEditorProps {
  ariaLabel: string;
  attachmentStatus?: (attachmentId: string) => Promise<GenericAttachmentStatusRecord>;
  disabled?: boolean;
  imageStatus?: (attachmentId: string) => Promise<ImageStatusRecord>;
  onChange: (value: string) => void;
  onImageError?: (error: unknown) => void;
  onPendingImagesChange?: (pending: boolean) => void;
  openAttachmentExternal?: (attachmentId: string) => Promise<void>;
  openPdfExternal?: (attachmentId: string) => Promise<void>;
  pasteImage?: () => Promise<ImageRecord>;
  pdfStatus?: (attachmentId: string) => Promise<PdfStatusRecord>;
  pickAttachment?: () => Promise<SelectedAttachmentRecord | null>;
  pickImage?: () => Promise<ImageRecord | null>;
  pickPdf?: () => Promise<PdfRecord | null>;
  placeholder?: string;
  revealAttachmentInFinder?: (attachmentId: string) => Promise<void>;
  retryImageOcr?: (attachmentId: string) => Promise<ImageStatusRecord>;
  retryPdfExtraction?: (attachmentId: string) => Promise<PdfStatusRecord>;
  value: string;
}

export const KoshBlockNoteEditor = forwardRef<KoshBlockNoteEditorHandle, KoshBlockNoteEditorProps>(
  function KoshBlockNoteEditor(properties, ref) {
    const { appearance } = useAppearance();
    const theme = useResolvedTheme(appearance);
    const propertiesRef = useRef(properties);
    propertiesRef.current = properties;
    const initialValue = useRef(properties.value).current;
    const initialPlaceholder = useRef(properties.placeholder).current;
    const lastEmittedValue = useRef(initialValue);
    const lastPropertyValue = useRef(initialValue);
    const pendingExternalValue = useRef<string | undefined>(undefined);
    const replacingValue = useRef(false);
    const literalSlashPending = useRef(false);
    if (properties.value !== lastPropertyValue.current) {
      lastPropertyValue.current = properties.value;
      if (properties.value !== lastEmittedValue.current) {
        pendingExternalValue.current = properties.value;
      }
    }
    const capabilities = useRef({
      attachmentStatus: Boolean(properties.attachmentStatus),
      imageStatus: Boolean(properties.imageStatus),
      openAttachmentExternal: Boolean(properties.openAttachmentExternal),
      openPdfExternal: Boolean(properties.openPdfExternal),
      pasteImage: Boolean(properties.pasteImage),
      pdfStatus: Boolean(properties.pdfStatus),
      pickAttachment: Boolean(properties.pickAttachment),
      pickImage: Boolean(properties.pickImage),
      pickPdf: Boolean(properties.pickPdf),
      revealAttachmentInFinder: Boolean(properties.revealAttachmentInFinder),
      retryImageOcr: Boolean(properties.retryImageOcr),
      retryPdfExtraction: Boolean(properties.retryPdfExtraction),
    }).current;
    const editor = useCreateBlockNote({
      schema: koshBlockNoteSchema,
      initialContent: markdownToKoshBlocks(initialValue),
      placeholders: { default: initialPlaceholder },
      tabBehavior: "prefer-indent",
      domAttributes: {
        editor: {
          "aria-label": properties.ariaLabel,
          "aria-multiline": "true",
          class: "kosh-blocknote-content",
          role: "textbox",
        },
      },
    });
    const mediaController = useMemo(
      () =>
        createBlockNoteMediaController(editor, {
          onError: (error) => propertiesRef.current.onImageError?.(error),
          onPendingChange: (pending) => propertiesRef.current.onPendingImagesChange?.(pending),
        }),
      [editor],
    );
    const mediaActions = useMemo<KoshMediaActions>(
      () => ({
        attachmentStatus: capabilities.attachmentStatus
          ? (attachmentId) => propertiesRef.current.attachmentStatus!(attachmentId)
          : undefined,
        imageStatus: capabilities.imageStatus
          ? (attachmentId) => propertiesRef.current.imageStatus!(attachmentId)
          : undefined,
        onError: (error) => propertiesRef.current.onImageError?.(error),
        openAttachmentExternal: capabilities.openAttachmentExternal
          ? (attachmentId) => propertiesRef.current.openAttachmentExternal!(attachmentId)
          : undefined,
        openPdfExternal: capabilities.openPdfExternal
          ? (attachmentId) => propertiesRef.current.openPdfExternal!(attachmentId)
          : undefined,
        pdfStatus: capabilities.pdfStatus
          ? (attachmentId) => propertiesRef.current.pdfStatus!(attachmentId)
          : undefined,
        pickReplacement: capabilities.pickAttachment
          ? () => mediaController.track(() => propertiesRef.current.pickAttachment!())
          : undefined,
        revealAttachmentInFinder: capabilities.revealAttachmentInFinder
          ? (attachmentId) => propertiesRef.current.revealAttachmentInFinder!(attachmentId)
          : undefined,
        retryImageOcr: capabilities.retryImageOcr
          ? (attachmentId) => propertiesRef.current.retryImageOcr!(attachmentId)
          : undefined,
        retryPdfExtraction: capabilities.retryPdfExtraction
          ? (attachmentId) => propertiesRef.current.retryPdfExtraction!(attachmentId)
          : undefined,
      }),
      [capabilities, mediaController],
    );
    const slashItems = useMemo(
      () => restrictedSlashItems(editor, mediaController, propertiesRef, capabilities),
      [capabilities, editor, mediaController],
    );

    useEffect(() => {
      mediaController.activate();
      return () => mediaController.dispose();
    }, [mediaController]);

    useEffect(() => {
      editor.isEditable = !properties.disabled;
      editor.domElement?.setAttribute("aria-label", properties.ariaLabel);
      editor.domElement?.setAttribute("aria-disabled", String(Boolean(properties.disabled)));
    }, [editor, properties.ariaLabel, properties.disabled]);

    useEffect(() => {
      const nextValue = pendingExternalValue.current;
      if (nextValue === undefined) return;
      const current = koshBlocksToMarkdown(editor.document);
      if (current !== nextValue) {
        replacingValue.current = true;
        try {
          editor.transact((transaction) => {
            editor.replaceBlocks(editor.document, markdownToKoshBlocks(nextValue));
            transaction.setMeta("addToHistory", false);
          });
          editor.replaceExtension("history", HistoryExtension());
        } finally {
          replacingValue.current = false;
        }
      }
      lastEmittedValue.current = nextValue;
      pendingExternalValue.current = undefined;
    }, [editor, properties.value]);

    useImperativeHandle(
      ref,
      () => ({
        focus: () => editor.focus(),
        insertAttachments: (attachments) => mediaController.insert(attachments),
        insertImages: (images) =>
          mediaController.insert(images.map((record) => ({ recordKind: "IMAGE", record }))),
        insertPdfs: (pdfs) =>
          mediaController.insert(pdfs.map((record) => ({ recordKind: "PDF", record }))),
      }),
      [editor, mediaController],
    );

    return (
      <MantineProvider forceColorScheme={theme}>
        <KoshEditorInteractionProvider disabled={Boolean(properties.disabled)}>
          <KoshMediaActionsProvider actions={mediaActions}>
            <div
              aria-disabled={properties.disabled || undefined}
              className="kosh-rich-text-editor kosh-blocknote-editor"
              data-testid={`${properties.ariaLabel.toLowerCase().replace(/\s+/gu, "-")}-editor`}
              onPasteCapture={(event) => {
                if (properties.disabled || !capabilities.pasteImage) return;
                mediaController.handleImagePaste(event.nativeEvent, async () => ({
                  recordKind: "IMAGE",
                  record: await propertiesRef.current.pasteImage!(),
                }));
              }}
              onKeyDownCapture={(event) => {
                if (event.key !== "Escape" || !literalSlashPending.current) return;
                window.requestAnimationFrame(() => {
                  const markdown = koshBlocksToMarkdown(editor.document);
                  if (markdown.trim() !== "/") return;
                  literalSlashPending.current = false;
                  lastEmittedValue.current = markdown;
                  if (markdown !== propertiesRef.current.value) {
                    propertiesRef.current.onChange(markdown);
                  }
                });
              }}
            >
              <BlockNoteView
                comments={false}
                editor={editor}
                emojiPicker={false}
                filePanel={false}
                formattingToolbar
                onChange={() => {
                  if (replacingValue.current || pendingExternalValue.current !== undefined) return;
                  const markdown = koshBlocksToMarkdown(editor.document);
                  if (markdown.trim() === "/" && !propertiesRef.current.value.trim()) {
                    literalSlashPending.current = true;
                    return;
                  }
                  literalSlashPending.current = false;
                  lastEmittedValue.current = markdown;
                  if (markdown !== propertiesRef.current.value) {
                    propertiesRef.current.onChange(markdown);
                  }
                }}
                slashMenu={false}
                sideMenu={false}
                tableHandles={false}
                theme={theme}
              >
                <SuggestionMenuController
                  getItems={async (query) => filterSuggestionItems(slashItems, query)}
                  triggerCharacter="/"
                />
                <SideMenuController sideMenu={KoshSideMenu} />
              </BlockNoteView>
            </div>
          </KoshMediaActionsProvider>
        </KoshEditorInteractionProvider>
      </MantineProvider>
    );
  },
);

function restrictedSlashItems(
  editor: KoshBlockNoteEditorInstance,
  mediaController: ReturnType<typeof createBlockNoteMediaController>,
  properties: React.RefObject<KoshBlockNoteEditorProps>,
  capabilities: Record<string, boolean>,
): DefaultReactSuggestionItem[] {
  const items: DefaultReactSuggestionItem[] = [
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
  ];
  if (capabilities.pickImage) {
    items.push(
      mediaItem(editor, mediaController, "Image", ["picture"], async () => {
        const record = await properties.current.pickImage!();
        return record ? { recordKind: "IMAGE", record } : null;
      }),
    );
  }
  if (capabilities.pickPdf) {
    items.push(
      mediaItem(editor, mediaController, "PDF", ["document"], async () => {
        const record = await properties.current.pickPdf!();
        return record ? { recordKind: "PDF", record } : null;
      }),
    );
  }
  if (capabilities.pickAttachment) {
    items.push(
      mediaItem(editor, mediaController, "File", ["attachment"], () =>
        properties.current.pickAttachment!(),
      ),
    );
  }
  return items;
}

function blockItem(
  editor: KoshBlockNoteEditorInstance,
  title: string,
  block: KoshBlockNotePartialBlock,
  aliases: string[],
): DefaultReactSuggestionItem {
  return {
    title,
    aliases,
    group: "Kosh blocks",
    onItemClick: () => insertOrUpdateBlockForSlashMenu(editor, block),
  };
}

function mediaItem(
  editor: KoshBlockNoteEditorInstance,
  controller: ReturnType<typeof createBlockNoteMediaController>,
  title: string,
  aliases: string[],
  ingest: () => Promise<SelectedAttachmentRecord | null>,
): DefaultReactSuggestionItem {
  return {
    title,
    aliases: [...aliases, "upload"],
    group: "Kosh media",
    onItemClick: () => {
      insertOrUpdateBlockForSlashMenu(editor, { type: "paragraph" });
      void controller.begin(`Adding ${title.toLowerCase()}`, ingest);
    },
  };
}

function KoshSideMenu(properties: SideMenuProps) {
  return (
    <SideMenu {...properties}>
      <KoshDragHandleButton />
    </SideMenu>
  );
}

function KoshDragHandleButton() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshBlockNoteSchema);
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
              const target = editor.getBlock(hoveredBlock.id) ?? editor.document[0];
              if (target) editor.setTextCursorPosition(target, "start");
              editor.focus();
            });
          }}
          onDragStart={(event) => sideMenu.blockDragStart(event, hoveredBlock)}
        />
      </Components.Generic.Menu.Trigger>
      <DragHandleMenu>
        <KoshMoveBlockItem direction="up" />
        <KoshMoveBlockItem direction="down" />
        <KoshRemoveBlockItem />
      </DragHandleMenu>
    </Components.Generic.Menu.Root>
  );
}

function KoshMoveBlockItem({ direction }: { direction: "down" | "up" }) {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshBlockNoteSchema);
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
  const editor = useBlockNoteEditor(koshBlockNoteSchema);
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;
  return (
    <Components.Generic.Menu.Item
      className="bn-menu-item"
      onClick={() => {
        const selected = editor.getSelection()?.blocks;
        const blocks = selected?.some((block) => block.id === hoveredBlock.id)
          ? selected
          : [hoveredBlock];
        const index = Math.max(
          0,
          editor.document.findIndex((block) => block.id === blocks[0]?.id),
        );
        editor.removeBlocks(blocks);
        requestAnimationFrame(() => {
          const target = editor.document[Math.min(index, editor.document.length - 1)];
          if (target) editor.setTextCursorPosition(target, "start");
          editor.focus();
        });
      }}
    >
      Delete selected blocks
    </Components.Generic.Menu.Item>
  );
}

function useResolvedTheme(appearance: "DARK" | "LIGHT" | "SYSTEM"): "dark" | "light" {
  const [systemDark, setSystemDark] = useState(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => setSystemDark(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  if (appearance === "DARK") return "dark";
  if (appearance === "LIGHT") return "light";
  return systemDark ? "dark" : "light";
}
