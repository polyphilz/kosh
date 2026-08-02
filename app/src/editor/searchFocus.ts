import { createExtension } from "@blocknote/core";
import { Plugin, PluginKey } from "prosemirror-state";
import { Decoration, DecorationSet } from "prosemirror-view";
import type { KoshBlockNoteEditor } from "./schema";

const SEARCH_FOCUS_KEY = new PluginKey<ReadonlySet<string>>("kosh-search-focus");

export const KoshSearchFocusExtension = createExtension({
  key: "koshSearchFocus",
  prosemirrorPlugins: [
    new Plugin<ReadonlySet<string>>({
      key: SEARCH_FOCUS_KEY,
      state: {
        init: () => new Set<string>(),
        apply(transaction, current) {
          return transaction.getMeta(SEARCH_FOCUS_KEY) ?? current;
        },
      },
      props: {
        decorations(state) {
          const focusedIds = SEARCH_FOCUS_KEY.getState(state);
          if (!focusedIds?.size) return DecorationSet.empty;
          const decorations: Decoration[] = [];
          state.doc.descendants((node, position) => {
            const id = typeof node.attrs.id === "string" ? node.attrs.id : null;
            if (id && focusedIds.has(id)) {
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
): void {
  const transaction = editor.prosemirrorView.state.tr.setMeta(SEARCH_FOCUS_KEY, new Set(blockIds));
  editor.prosemirrorView.dispatch(transaction);
}
