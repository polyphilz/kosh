import { createExtension, getNodeById } from "@blocknote/core";
import { Fragment, type Node as ProseMirrorNode, type ResolvedPos, Slice } from "prosemirror-model";
import { Plugin, PluginKey, Selection } from "prosemirror-state";
import { Decoration, DecorationSet } from "prosemirror-view";
import type { KoshBlockNoteEditor } from "./schema";

const EMPTY_GUTTER_SELECTION = new Set<string>();
const GUTTER_SELECTION_KEY = new PluginKey<ReadonlySet<string>>("kosh-gutter-selection");

class KoshBlockRangeSelection extends Selection {
  readonly nodes: ProseMirrorNode[];

  constructor($anchor: ResolvedPos, $head: ResolvedPos) {
    super($anchor, $head);
    const parent = $anchor.node();
    this.nodes = [];
    $anchor.doc.nodesBetween($anchor.pos, $head.pos, (node, _position, nodeParent) => {
      if (nodeParent !== null && nodeParent.eq(parent)) {
        this.nodes.push(node);
        return false;
      }
      return undefined;
    });
  }

  static create(document: ProseMirrorNode, from: number, to: number): KoshBlockRangeSelection {
    return new KoshBlockRangeSelection(document.resolve(from), document.resolve(to));
  }

  content(): Slice {
    return new Slice(Fragment.from(this.nodes), 0, 0);
  }

  eq(selection: Selection): boolean {
    return (
      selection instanceof KoshBlockRangeSelection &&
      this.from === selection.from &&
      this.to === selection.to &&
      this.nodes.length === selection.nodes.length &&
      this.nodes.every((node, index) => node.eq(selection.nodes[index]!))
    );
  }

  map(document: ProseMirrorNode, mapping: Parameters<Selection["map"]>[1]): Selection {
    const fromResult = mapping.mapResult(this.from);
    const toResult = mapping.mapResult(this.to);
    if (toResult.deleted) return Selection.near(document.resolve(fromResult.pos));
    if (fromResult.deleted) return Selection.near(document.resolve(toResult.pos));
    return KoshBlockRangeSelection.create(document, fromResult.pos, toResult.pos);
  }

  toJSON(): { anchor: number; head: number; type: string } {
    return { type: "kosh-block-range", anchor: this.anchor, head: this.head };
  }
}

export const KoshGutterSelectionExtension = createExtension({
  key: "koshGutterSelection",
  prosemirrorPlugins: [
    new Plugin<ReadonlySet<string>>({
      key: GUTTER_SELECTION_KEY,
      state: {
        init: () => EMPTY_GUTTER_SELECTION,
        apply(transaction, current) {
          const explicit = transaction.getMeta(GUTTER_SELECTION_KEY) as
            | ReadonlySet<string>
            | undefined;
          if (explicit) return explicit;
          if (
            transaction.selectionSet &&
            !(transaction.selection instanceof KoshBlockRangeSelection)
          ) {
            return EMPTY_GUTTER_SELECTION;
          }
          return current;
        },
      },
      props: {
        decorations(state) {
          const selectedBlockIds = GUTTER_SELECTION_KEY.getState(state);
          if (!selectedBlockIds?.size) return DecorationSet.empty;
          const decorations: Decoration[] = [];
          state.doc.descendants((node, position) => {
            const id = typeof node.attrs.id === "string" ? node.attrs.id : null;
            if (!id || !selectedBlockIds.has(id)) return;
            decorations.push(
              Decoration.node(position, position + node.nodeSize, {
                "data-kosh-gutter-selected": "true",
              }),
            );
            return false;
          });
          return DecorationSet.create(state.doc, decorations);
        },
      },
    }),
  ],
});

export function setGutterBlockSelection(
  editor: KoshBlockNoteEditor,
  blockIds: readonly string[],
): boolean {
  const first = getNodeById(blockIds[0] ?? "", editor.prosemirrorView.state.doc);
  const last = getNodeById(blockIds.at(-1) ?? "", editor.prosemirrorView.state.doc);
  if (!first || !last) return false;
  const selectedBlockIds = new Set(blockIds);
  const transaction = editor.prosemirrorView.state.tr
    .setSelection(
      KoshBlockRangeSelection.create(
        editor.prosemirrorView.state.doc,
        first.posBeforeNode,
        last.posBeforeNode + last.node.nodeSize,
      ),
    )
    .setMeta(GUTTER_SELECTION_KEY, selectedBlockIds);
  editor.prosemirrorView.dispatch(transaction);
  editor.focus();
  return true;
}
