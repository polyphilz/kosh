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
import { installSpikeBridge } from "./bridge";
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
  const slashItems = useMemo(() => restrictedSlashItems(editor), [editor]);

  useEffect(
    () =>
      installSpikeBridge(editor, {
        blocks: supportedSpikeBlockTypes,
        inlineContent: supportedSpikeInlineTypes,
        styles: supportedSpikeStyleTypes,
      }),
    [editor],
  );

  return (
    <MantineProvider forceColorScheme={theme}>
      <main className="kosh-blocknote-spike" data-theme={theme}>
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
    </MantineProvider>
  );
}

function KoshSpikeDragMenu() {
  return (
    <DragHandleMenu>
      <KoshRemoveBlockItem />
    </DragHandleMenu>
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

function restrictedSlashItems(editor: KoshSpikeEditor): DefaultReactSuggestionItem[] {
  return [
    blockItem(editor, "Paragraph", { type: "paragraph" }, ["text", "body"]),
    blockItem(editor, "Heading 1", { type: "heading", props: { level: 1 } }, ["h1"]),
    blockItem(editor, "Heading 2", { type: "heading", props: { level: 2 } }, ["h2"]),
    blockItem(editor, "Heading 3", { type: "heading", props: { level: 3 } }, ["h3"]),
    blockItem(editor, "Bullet list", { type: "bulletListItem" }, ["unordered", "ul"]),
    blockItem(editor, "Ordered list", { type: "numberedListItem" }, ["numbered", "ol"]),
    blockItem(editor, "Code block", { type: "codeBlock", props: { language: "text" } }, ["fence"]),
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
