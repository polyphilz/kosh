import { createExtension } from "@blocknote/core";
import { Plugin, PluginKey, TextSelection } from "prosemirror-state";
import { Decoration, DecorationSet } from "prosemirror-view";
import type { Node as ProseMirrorNode } from "prosemirror-model";
import type { KoshBlockNoteEditor } from "./schema";

export interface SearchFocusInlineRange {
  blockId: string;
  endChar: number;
  startChar: number;
}

interface SearchFocusTarget {
  blockIds: ReadonlySet<string>;
  inlineRange: SearchFocusInlineRange | null;
}

const EMPTY_SEARCH_FOCUS: SearchFocusTarget = {
  blockIds: new Set<string>(),
  inlineRange: null,
};

const SEARCH_FOCUS_KEY = new PluginKey<SearchFocusTarget>("kosh-search-focus");

export const KoshSearchFocusExtension = createExtension({
  key: "koshSearchFocus",
  prosemirrorPlugins: [
    new Plugin<SearchFocusTarget>({
      key: SEARCH_FOCUS_KEY,
      state: {
        init: () => EMPTY_SEARCH_FOCUS,
        apply(transaction, current) {
          return transaction.getMeta(SEARCH_FOCUS_KEY) ?? current;
        },
      },
      props: {
        decorations(state) {
          const target = SEARCH_FOCUS_KEY.getState(state);
          if (!target?.blockIds.size) return DecorationSet.empty;
          const decorations: Decoration[] = [];
          state.doc.descendants((node, position) => {
            const id = typeof node.attrs.id === "string" ? node.attrs.id : null;
            if (!id || !target.blockIds.has(id)) return;
            if (target.inlineRange?.blockId === id) {
              const range = resolveInlineRange(node, position, target.inlineRange);
              if (range) {
                decorations.push(
                  Decoration.inline(range.from, range.to, {
                    "data-kosh-search-hit": "true",
                  }),
                );
              }
            } else {
              decorations.push(
                Decoration.node(position, position + node.nodeSize, {
                  "data-kosh-search-hit": "true",
                }),
              );
            }
          });
          return DecorationSet.create(state.doc, decorations);
        },
      },
    }),
  ],
});

export function setSearchFocusBlocks(
  editor: KoshBlockNoteEditor,
  blockIds: readonly string[],
  inlineRange: SearchFocusInlineRange | null = null,
): boolean {
  const target: SearchFocusTarget = { blockIds: new Set(blockIds), inlineRange };
  let transaction = editor.prosemirrorView.state.tr.setMeta(SEARCH_FOCUS_KEY, target);
  let selectionPosition: number | null = null;
  if (inlineRange) {
    transaction.doc.descendants((node, position) => {
      if (selectionPosition !== null || node.attrs.id !== inlineRange.blockId) return;
      selectionPosition = resolveInlineRange(node, position, inlineRange)?.from ?? null;
    });
    if (selectionPosition !== null) {
      transaction = transaction.setSelection(
        TextSelection.near(transaction.doc.resolve(selectionPosition)),
      );
    }
  }
  editor.prosemirrorView.dispatch(transaction);
  return !inlineRange || selectionPosition !== null;
}

function resolveInlineRange(
  block: ProseMirrorNode,
  blockPosition: number,
  range: SearchFocusInlineRange,
): { from: number; to: number } | null {
  if (range.startChar < 0 || range.endChar <= range.startChar) return null;
  const textNodes: Array<{ position: number; text: string }> = [];
  block.descendants((node, position) => {
    if (node.isText && node.text) {
      textNodes.push({ position: blockPosition + 1 + position, text: node.text });
    }
  });
  let consumedCharacters = 0;
  let from: number | null = null;
  let to: number | null = null;
  for (const textNode of textNodes) {
    const characters = [...textNode.text];
    const nodeStart = consumedCharacters;
    const nodeEnd = nodeStart + characters.length;
    if (from === null && range.startChar >= nodeStart && range.startChar <= nodeEnd) {
      from = textNode.position + characters.slice(0, range.startChar - nodeStart).join("").length;
    }
    if (range.endChar >= nodeStart && range.endChar <= nodeEnd) {
      to = textNode.position + characters.slice(0, range.endChar - nodeStart).join("").length;
      break;
    }
    consumedCharacters = nodeEnd;
  }
  return from !== null && to !== null && from < to ? { from, to } : null;
}
