import {
  filterSuggestionItems,
  HistoryExtension,
  insertOrUpdateBlockForSlashMenu,
  SideMenuExtension,
  SuggestionMenu as SuggestionMenuExtension,
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
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type {
  ImageRecord,
  ImageStatusRecord,
  SelectedAttachmentRecord,
} from "../backend/contracts";
import { useAppearance } from "../components/Appearance";
import {
  clearFindInNote as clearEditorFind,
  findInNote as findEditorText,
  type FindInNoteResult,
  KoshFindInNoteExtension,
  moveFindInNote as moveEditorFind,
} from "./findInNote";
import { KoshEditorInteractionProvider, useKoshEditorDisabled } from "./interactionState";
import {
  clearGutterBlockSelection,
  KoshGutterSelectionExtension,
  setGutterBlockSelection,
} from "./gutterSelection";
import { parseKoshDocument, serializeKoshDocument } from "./document";
import { koshBlocksToMarkdown } from "./markdownAdapter";
import { KoshMediaActionsProvider, type KoshMediaActions } from "./mediaBlocks";
import { createBlockNoteMediaController } from "./mediaController";
import { KoshSearchFocusExtension, setSearchFocusBlocks } from "./searchFocus";
import {
  koshBlockNoteSchema,
  type KoshBlockNoteEditor as KoshBlockNoteEditorInstance,
  type KoshBlockNotePartialBlock,
} from "./schema";
import { KOSH_WRITING_ASSISTANCE_ATTRIBUTES } from "./writingAssistance";

export const KOSH_NOTE_PLACEHOLDER = "Write something or press '/' for commands";

export interface KoshBlockNoteEditorHandle {
  clearFindInNote: () => void;
  clearSearchFocus: () => void;
  findInNote: (query: string, activeIndex?: number) => FindInNoteResult;
  focus: () => void;
  focusBlock: (blockId: string) => boolean;
  hasBlock: (blockId: string) => boolean;
  isSuggestionMenuOpen: () => boolean;
  insertAttachments: (attachments: SelectedAttachmentRecord[]) => void;
  insertImages: (images: ImageRecord[]) => void;
  moveFindInNote: (direction: "next" | "previous") => FindInNoteResult;
}

export interface KoshBlockNoteEditorProps {
  ariaLabel: string;
  disabled?: boolean;
  imageStatus?: (attachmentId: string) => Promise<ImageStatusRecord>;
  onChange: (documentJson: string, bodyMarkdown: string) => void;
  onFindStateChange?: () => void;
  onImageError?: (error: unknown) => void;
  onPendingImagesChange?: (pending: boolean) => void;
  openAttachmentExternal?: (attachmentId: string) => Promise<void>;
  pasteImage?: () => Promise<ImageRecord>;
  pickAttachment?: () => Promise<SelectedAttachmentRecord | null>;
  pickImage?: () => Promise<ImageRecord | null>;
  placeholder?: string;
  revealAttachmentInFinder?: (attachmentId: string) => Promise<void>;
  retryImageOcr?: (attachmentId: string) => Promise<ImageStatusRecord>;
  selectionRail?: boolean;
  variant?: "default" | "page";
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
    const slashCommandSelected = useRef(false);
    const suggestionMenuOpen = useRef(false);
    const searchFocusBlockIds = useRef<string[]>([]);
    if (properties.value !== lastPropertyValue.current) {
      lastPropertyValue.current = properties.value;
      if (properties.value !== lastEmittedValue.current) {
        pendingExternalValue.current = properties.value;
      }
    }
    const capabilities = useRef({
      imageStatus: Boolean(properties.imageStatus),
      openAttachmentExternal: Boolean(properties.openAttachmentExternal),
      pasteImage: Boolean(properties.pasteImage),
      pickAttachment: Boolean(properties.pickAttachment),
      pickImage: Boolean(properties.pickImage),
      revealAttachmentInFinder: Boolean(properties.revealAttachmentInFinder),
      retryImageOcr: Boolean(properties.retryImageOcr),
    }).current;
    const editor = useCreateBlockNote({
      schema: koshBlockNoteSchema,
      initialContent: parseKoshDocument(initialValue),
      placeholders: { default: initialPlaceholder },
      tabBehavior: "prefer-indent",
      extensions: [KoshFindInNoteExtension, KoshGutterSelectionExtension, KoshSearchFocusExtension],
      domAttributes: {
        editor: {
          ...KOSH_WRITING_ASSISTANCE_ATTRIBUTES,
          "aria-label": properties.ariaLabel,
          "aria-multiline": "true",
          class: "kosh-blocknote-content",
          role: "textbox",
        },
      },
    });
    if (lastEmittedValue.current === initialValue) {
      lastEmittedValue.current = serializeKoshDocument(editor.document);
    }
    const mediaController = useMemo(
      () =>
        createBlockNoteMediaController(editor, {
          onError: (error) => propertiesRef.current.onImageError?.(error),
          onPendingChange: (pending) => propertiesRef.current.onPendingImagesChange?.(pending),
        }),
      [editor],
    );
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
    const mediaActions = useMemo<KoshMediaActions>(
      () => ({
        imageStatus: capabilities.imageStatus
          ? (attachmentId) => propertiesRef.current.imageStatus!(attachmentId)
          : undefined,
        onError: (error) => propertiesRef.current.onImageError?.(error),
        openAttachmentExternal: capabilities.openAttachmentExternal
          ? (attachmentId) => propertiesRef.current.openAttachmentExternal!(attachmentId)
          : undefined,
        revealAttachmentInFinder: capabilities.revealAttachmentInFinder
          ? (attachmentId) => propertiesRef.current.revealAttachmentInFinder!(attachmentId)
          : undefined,
        retryImageOcr: capabilities.retryImageOcr
          ? (attachmentId) => propertiesRef.current.retryImageOcr!(attachmentId)
          : undefined,
      }),
      [capabilities, mediaController],
    );
    const slashItems = useMemo(
      () => restrictedSlashItems(editor, mediaController, propertiesRef, capabilities),
      [capabilities, editor, mediaController],
    );
    const emitUserChange = useCallback((documentJson: string, bodyMarkdown: string) => {
      if (documentJson === lastEmittedValue.current) return;
      lastEmittedValue.current = documentJson;
      propertiesRef.current.onChange(documentJson, bodyMarkdown);
    }, []);
    const settleLiteralSlash = useCallback(() => {
      const commandWasSelected = slashCommandSelected.current;
      slashCommandSelected.current = false;
      if (!literalSlashPending.current) return;
      literalSlashPending.current = false;
      if (commandWasSelected) return;
      const markdown = koshBlocksToMarkdown(editor.document);
      if (markdown.trim() !== "/") return;
      emitUserChange(serializeKoshDocument(editor.document), markdown);
    }, [editor, emitUserChange]);

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
      const current = serializeKoshDocument(editor.document);
      if (current !== nextValue) {
        replacingValue.current = true;
        try {
          editor.transact((transaction) => {
            editor.replaceBlocks(editor.document, parseKoshDocument(nextValue));
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
        clearFindInNote: () => clearEditorFind(editor),
        clearSearchFocus: () => clearSearchFocus(editor, searchFocusBlockIds),
        findInNote: (query, activeIndex) => findEditorText(editor, query, activeIndex),
        focus: () => editor.focus(),
        focusBlock: (blockId) => focusBlock(editor, blockId, searchFocusBlockIds),
        hasBlock: (blockId) => flattenBlocks(editor.document).some((block) => block.id === blockId),
        isSuggestionMenuOpen: () => suggestionMenuOpen.current,
        insertAttachments: (attachments) => mediaController.insert(attachments),
        insertImages: (images) =>
          mediaController.insert(images.map((record) => ({ recordKind: "IMAGE", record }))),
        moveFindInNote: (direction) => moveEditorFind(editor, direction),
      }),
      [editor, mediaController],
    );

    useEffect(() => () => clearSearchFocus(editor, searchFocusBlockIds), [editor]);

    return (
      <MantineProvider forceColorScheme={theme}>
        <KoshEditorInteractionProvider disabled={Boolean(properties.disabled)}>
          <KoshMediaActionsProvider actions={mediaActions}>
            <div
              aria-disabled={properties.disabled || undefined}
              className={`kosh-rich-text-editor kosh-blocknote-editor${
                properties.variant === "page" ? " kosh-blocknote-editor--page" : ""
              }`}
              data-testid={`${properties.ariaLabel.toLowerCase().replace(/\s+/gu, "-")}-editor`}
              onPasteCapture={(event) => {
                if (properties.disabled || !capabilities.pasteImage) return;
                mediaController.handleImagePaste(event.nativeEvent, async () => ({
                  recordKind: "IMAGE",
                  record: await propertiesRef.current.pasteImage!(),
                }));
              }}
            >
              {properties.variant === "page" && properties.selectionRail && (
                <KoshGutterSelectionRail disabled={Boolean(properties.disabled)} editor={editor} />
              )}
              <BlockNoteView
                comments={false}
                editor={editor}
                emojiPicker={false}
                filePanel={false}
                formattingToolbar
                onChange={() => {
                  propertiesRef.current.onFindStateChange?.();
                  if (replacingValue.current || pendingExternalValue.current !== undefined) return;
                  const markdown = koshBlocksToMarkdown(editor.document);
                  const previousMarkdown = koshBlocksToMarkdown(
                    parseKoshDocument(propertiesRef.current.value),
                  );
                  if (markdown.trim() === "/" && !previousMarkdown.trim()) {
                    literalSlashPending.current = true;
                    return;
                  }
                  literalSlashPending.current = false;
                  emitUserChange(serializeKoshDocument(editor.document), markdown);
                }}
                slashMenu={false}
                sideMenu={false}
                tableHandles={false}
                theme={theme}
              >
                <SuggestionMenuController
                  getItems={async (query) => filterSuggestionItems(slashItems, query)}
                  onItemClick={(item) => {
                    slashCommandSelected.current = true;
                    item.onItemClick();
                  }}
                  triggerCharacter="/"
                />
                <KoshSlashMenuLifecycle
                  onClosed={settleLiteralSlash}
                  onOpenChange={(open) => {
                    suggestionMenuOpen.current = open;
                  }}
                />
                <SideMenuController
                  floatingUIOptions={sideMenuFloatingUIOptions}
                  sideMenu={KoshSideMenu}
                />
              </BlockNoteView>
            </div>
          </KoshMediaActionsProvider>
        </KoshEditorInteractionProvider>
      </MantineProvider>
    );
  },
);

function KoshGutterSelectionRail({
  disabled,
  editor,
}: {
  disabled: boolean;
  editor: KoshBlockNoteEditorInstance;
}) {
  const drag = useRef<{
    anchorBlockId: string;
    anchorOffsetY: number;
    currentX: number;
    currentY: number;
    dragging: boolean;
    pointerId: number;
    scrollFrame: number | null;
    startX: number;
    startY: number;
  } | null>(null);
  const [marquee, setMarquee] = useState<GutterMarquee | null>(null);

  const installMarqueeSelection = useCallback(
    (bounds: GutterMarquee) => {
      const root = editor.domElement;
      if (!root) return;
      const blockIds = topLevelBlockElements(root)
        .filter((element) => intersectsMarquee(element.getBoundingClientRect(), bounds))
        .map((element) => element.dataset.id)
        .filter((id): id is string => Boolean(id));
      if (blockIds.length > 0) {
        setGutterBlockSelection(editor, blockIds);
      } else {
        clearGutterBlockSelection(editor);
      }
    },
    [editor],
  );

  const updateMarquee = useCallback(
    (clientX: number, clientY: number) => {
      const activeDrag = drag.current;
      if (!activeDrag) return;
      const root = editor.domElement;
      const anchor = root
        ? topLevelBlockElements(root).find(
            (element) => element.dataset.id === activeDrag.anchorBlockId,
          )
        : undefined;
      if (!anchor) return;
      const anchorY = anchor.getBoundingClientRect().top + activeDrag.anchorOffsetY;
      const bounds = marqueeBetween(activeDrag.startX, anchorY, clientX, clientY);
      setMarquee(bounds);
      installMarqueeSelection(bounds);
    },
    [editor, installMarqueeSelection],
  );

  const stopAutoScroll = useCallback(() => {
    const activeDrag = drag.current;
    if (activeDrag?.scrollFrame !== null && activeDrag?.scrollFrame !== undefined) {
      window.cancelAnimationFrame(activeDrag.scrollFrame);
      activeDrag.scrollFrame = null;
    }
  }, []);

  const scheduleAutoScroll = useCallback(() => {
    const activeDrag = drag.current;
    if (!activeDrag?.dragging || activeDrag.scrollFrame !== null) return;
    const step = () => {
      const current = drag.current;
      if (!current?.dragging) return;
      current.scrollFrame = null;
      const delta = marqueeScrollDelta(current.currentY, window.innerHeight);
      if (delta === 0) return;
      const before = window.scrollY;
      window.scrollBy(0, delta);
      if (window.scrollY === before) return;
      updateMarquee(current.currentX, current.currentY);
      current.scrollFrame = window.requestAnimationFrame(step);
    };
    activeDrag.scrollFrame = window.requestAnimationFrame(step);
  }, [updateMarquee]);

  useEffect(() => stopAutoScroll, [stopAutoScroll]);

  return (
    <div
      aria-hidden="true"
      className="kosh-blocknote-gutter-selection-rail"
      data-testid="note-gutter-selection-rail"
      onPointerCancel={(event) => {
        stopAutoScroll();
        drag.current = null;
        setMarquee(null);
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
      }}
      onPointerDown={(event) => {
        if (disabled || event.button !== 0) return;
        const root = editor.domElement;
        const block = root
          ? blockElementAtY(topLevelBlockElements(root), event.clientY)
          : undefined;
        const blockId = block?.dataset.id;
        if (!blockId) return;
        event.preventDefault();
        drag.current = {
          anchorBlockId: blockId,
          anchorOffsetY: event.clientY - block.getBoundingClientRect().top,
          currentX: event.clientX,
          currentY: event.clientY,
          dragging: false,
          pointerId: event.pointerId,
          scrollFrame: null,
          startX: event.clientX,
          startY: event.clientY,
        };
        setMarquee(null);
        event.currentTarget.setPointerCapture(event.pointerId);
        setGutterBlockSelection(editor, [blockId]);
      }}
      onPointerMove={(event) => {
        const activeDrag = drag.current;
        if (!activeDrag || activeDrag.pointerId !== event.pointerId || (event.buttons & 1) === 0) {
          return;
        }
        if (
          !activeDrag.dragging &&
          Math.hypot(event.clientX - activeDrag.startX, event.clientY - activeDrag.startY) < 3
        ) {
          return;
        }
        event.preventDefault();
        activeDrag.currentX = event.clientX;
        activeDrag.currentY = event.clientY;
        activeDrag.dragging = true;
        updateMarquee(event.clientX, event.clientY);
        scheduleAutoScroll();
      }}
      onPointerUp={(event) => {
        const activeDrag = drag.current;
        if (!activeDrag || activeDrag.pointerId !== event.pointerId) return;
        event.preventDefault();
        if (activeDrag.dragging) {
          updateMarquee(event.clientX, event.clientY);
        } else {
          setGutterBlockSelection(editor, [activeDrag.anchorBlockId]);
        }
        stopAutoScroll();
        drag.current = null;
        setMarquee(null);
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
      }}
    >
      {marquee && (
        <div
          className="kosh-blocknote-gutter-selection-marquee"
          data-testid="note-gutter-selection-marquee"
          style={{
            height: marquee.height,
            left: marquee.left,
            top: marquee.top,
            width: marquee.width,
          }}
        />
      )}
    </div>
  );
}

interface GutterMarquee {
  height: number;
  left: number;
  top: number;
  width: number;
}

function marqueeBetween(startX: number, startY: number, endX: number, endY: number): GutterMarquee {
  return {
    height: Math.abs(endY - startY),
    left: Math.min(startX, endX),
    top: Math.min(startY, endY),
    width: Math.abs(endX - startX),
  };
}

function marqueeScrollDelta(clientY: number, viewportHeight: number): number {
  const edge = 56;
  const maximum = 18;
  if (clientY < edge) return -Math.min(maximum, Math.ceil((edge - clientY) / 3));
  if (clientY > viewportHeight - edge) {
    return Math.min(maximum, Math.ceil((clientY - (viewportHeight - edge)) / 3));
  }
  return 0;
}

function intersectsMarquee(bounds: DOMRect, marquee: GutterMarquee): boolean {
  return (
    marquee.left <= bounds.right &&
    marquee.left + marquee.width >= bounds.left &&
    marquee.top <= bounds.bottom &&
    marquee.top + marquee.height >= bounds.top
  );
}

function topLevelBlockElements(root: HTMLElement): HTMLElement[] {
  return [
    ...root.querySelectorAll<HTMLElement>(
      ":scope > .bn-block-group > .bn-block-outer:not(.bn-trailing-block)",
    ),
  ];
}

function blockElementAtY(blocks: HTMLElement[], clientY: number): HTMLElement | undefined {
  return blocks.find((element) => {
    const bounds = element.getBoundingClientRect();
    return clientY >= bounds.top && clientY <= bounds.bottom;
  });
}

function focusBlock(
  editor: KoshBlockNoteEditorInstance,
  blockId: string,
  focusedBlockIds: MutableRefObject<string[]>,
): boolean {
  clearSearchFocus(editor, focusedBlockIds);
  const block = flattenBlocks(editor.document).find((candidate) => candidate.id === blockId);
  if (!block) return false;
  editor.setTextCursorPosition(block, "start");
  editor.focus();
  focusedBlockIds.current = [block.id];
  if (!setSearchFocusBlocks(editor, focusedBlockIds.current)) {
    clearSearchFocus(editor, focusedBlockIds);
    return false;
  }
  const root = editor.domElement?.closest<HTMLElement>(".kosh-blocknote-editor");
  const element = root?.querySelector<HTMLElement>('[data-kosh-search-hit="true"]');
  element?.scrollIntoView({ behavior: "instant", block: "center" });
  return element !== null && element !== undefined;
}

function clearSearchFocus(
  editor: KoshBlockNoteEditorInstance,
  focusedBlockIds: MutableRefObject<string[]>,
): void {
  const hadFocus = focusedBlockIds.current.length > 0;
  focusedBlockIds.current = [];
  if (hadFocus) setSearchFocusBlocks(editor, []);
}

type SearchableBlock = {
  children?: SearchableBlock[];
  content?: unknown;
  id: string;
  props?: Record<string, unknown>;
  type: string;
};

function flattenBlocks(blocks: readonly unknown[]): SearchableBlock[] {
  const flattened: SearchableBlock[] = [];
  const visit = (value: unknown) => {
    if (!value || typeof value !== "object" || !("id" in value) || !("type" in value)) return;
    const block = value as SearchableBlock;
    flattened.push(block);
    for (const child of block.children ?? []) visit(child);
  };
  for (const block of blocks) visit(block);
  return flattened;
}

function KoshSlashMenuLifecycle({
  onClosed,
  onOpenChange,
}: {
  onClosed: () => void;
  onOpenChange: (open: boolean) => void;
}) {
  const state = useExtensionState(SuggestionMenuExtension);
  const open = Boolean(state?.show && state.triggerCharacter === "/");
  const wasOpen = useRef(false);
  useEffect(() => {
    if (wasOpen.current && !open) onClosed();
    wasOpen.current = open;
    onOpenChange(open);
    return () => onOpenChange(false);
  }, [onClosed, onOpenChange, open]);
  return null;
}

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
    onItemClick: () => {
      insertOrUpdateBlockForSlashMenu(editor, { type: "paragraph" });
      void controller.begin(`Adding ${title.toLowerCase()}`, ingest);
    },
  };
}

function KoshSideMenu(properties: SideMenuProps) {
  return (
    <SideMenu {...properties}>
      <KoshAddBlockButton />
      <KoshDragHandleButton />
    </SideMenu>
  );
}

function KoshAddBlockButton() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshBlockNoteSchema);
  const disabled = useKoshEditorDisabled();
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;
  return (
    <Components.SideMenu.Button
      className={`bn-button kosh-gutter-button kosh-gutter-button--add${
        disabled ? " bn-button--disabled" : ""
      }`}
      icon={<span aria-hidden>+</span>}
      label="Click to add below"
      onClick={(event) => {
        if (disabled) {
          event.preventDefault();
          return;
        }
        const inserted = editor.insertBlocks([{ type: "paragraph" }], hoveredBlock, "after")[0];
        if (!inserted) return;
        editor.setTextCursorPosition(inserted, "start");
        editor.focus();
      }}
    />
  );
}

function KoshDragHandleButton() {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshBlockNoteSchema);
  const disabled = useKoshEditorDisabled();
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
          className={`bn-button kosh-gutter-button kosh-gutter-button--drag${
            disabled ? " bn-button--disabled" : ""
          }`}
          draggable={!disabled}
          icon={
            <span aria-hidden className="kosh-gutter-dots">
              {Array.from({ length: 6 }, (_, index) => (
                <i key={index} />
              ))}
            </span>
          }
          label="Drag to move"
          onDragEnd={() => {
            if (disabled) return;
            sideMenu.blockDragEnd();
            requestAnimationFrame(() => {
              const target = editor.getBlock(hoveredBlock.id) ?? editor.document[0];
              if (target) editor.setTextCursorPosition(target, "start");
              editor.focus();
            });
          }}
          onDragStart={(event) => {
            if (!disabled) sideMenu.blockDragStart(event, hoveredBlock);
          }}
          onClick={(event) => {
            if (!disabled) return;
            event.preventDefault();
            event.stopPropagation();
          }}
        />
      </Components.Generic.Menu.Trigger>
      <DragHandleMenu>
        {!disabled && (
          <>
            <KoshMoveBlockItem direction="up" />
            <KoshMoveBlockItem direction="down" />
            <KoshRemoveBlockItem />
          </>
        )}
      </DragHandleMenu>
    </Components.Generic.Menu.Root>
  );
}

function KoshMoveBlockItem({ direction }: { direction: "down" | "up" }) {
  const Components = useComponentsContext()!;
  const editor = useBlockNoteEditor(koshBlockNoteSchema);
  const disabled = useKoshEditorDisabled();
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;
  return (
    <Components.Generic.Menu.Item
      className="bn-menu-item"
      onClick={() => {
        if (disabled) return;
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
  const disabled = useKoshEditorDisabled();
  const hoveredBlock = useExtensionState(SideMenuExtension, {
    editor,
    selector: (state) => state?.block,
  });
  if (!hoveredBlock) return null;
  return (
    <Components.Generic.Menu.Item
      className="bn-menu-item"
      onClick={() => {
        if (disabled) return;
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
