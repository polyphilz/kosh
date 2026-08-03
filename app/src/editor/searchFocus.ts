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
  const segments: EvidenceSegment[] = [];
  let evidenceStart = 0;
  block.descendants((node, position) => {
    const absolutePosition = blockPosition + 1 + position;
    const evidence = evidenceText(node);
    if (!evidence) return;
    const evidenceEnd = evidenceStart + [...evidence].length;
    segments.push({
      evidence,
      evidenceEnd,
      evidenceStart,
      from: absolutePosition,
      to: absolutePosition + node.nodeSize,
      atom: node.type.name === "inlineMath",
    });
    evidenceStart = evidenceEnd;
    return node.isText ? undefined : false;
  });
  const from = resolveEvidenceBoundary(segments, range.startChar, "start");
  const to = resolveEvidenceBoundary(segments, range.endChar, "end");
  return from !== null && to !== null && from < to ? { from, to } : null;
}

interface EvidenceSegment {
  atom: boolean;
  evidence: string;
  evidenceEnd: number;
  evidenceStart: number;
  from: number;
  to: number;
}

function evidenceText(node: ProseMirrorNode): string {
  if (node.isText) return node.text ?? "";
  if (node.type.name !== "inlineMath") return "";
  const latex = typeof node.attrs.latex === "string" ? node.attrs.latex : "";
  return latex ? `$${latex}$` : "";
}

function resolveEvidenceBoundary(
  segments: readonly EvidenceSegment[],
  offset: number,
  edge: "end" | "start",
): number | null {
  for (const segment of segments) {
    if (offset < segment.evidenceStart || offset > segment.evidenceEnd) continue;
    if (segment.atom) {
      if (offset === segment.evidenceEnd && edge === "start") continue;
      if (offset === segment.evidenceStart && edge === "end") continue;
      return edge === "start" ? segment.from : segment.to;
    }
    const relative = offset - segment.evidenceStart;
    return segment.from + [...segment.evidence].slice(0, relative).join("").length;
  }
  return null;
}
