import { createExtension } from "@blocknote/core";
import { closeHistory } from "prosemirror-history";
import { Plugin, TextSelection } from "prosemirror-state";
import type { KoshBlockNoteEditor } from "./schema";

const INLINE_MATH_INPUT = /\$\$([^$\n\uFFFC]+)\$\$$/u;

export const KoshInlineMathInputExtension = createExtension({
  key: "koshInlineMathInput",
  prosemirrorPlugins: [
    new Plugin({
      props: {
        handleTextInput(view, from, to, text) {
          if (text !== "$" || from !== to) return false;

          const $from = view.state.doc.resolve(from);
          if ($from.parent.type.spec.code || hasCodeMark($from.marks())) return false;

          const textBefore = $from.parent.textBetween(0, $from.parentOffset, undefined, "\uFFFC");
          const match = `${textBefore}${text}`.match(INLINE_MATH_INPUT);
          if (!match || match.index === undefined) return false;

          const latex = match[1];
          const inlineMath = view.state.schema.nodes.inlineMath;
          if (!latex?.trim() || !inlineMath || isEscaped(textBefore, match.index)) return false;

          const matchStart = from - (match[0].length - text.length);
          const node = inlineMath.create({ latex });
          view.dispatch(view.state.tr.insertText(text, from, to));

          const transaction = closeHistory(
            view.state.tr.replaceWith(matchStart, to + text.length, node),
          );
          transaction.setSelection(
            TextSelection.near(transaction.doc.resolve(matchStart + node.nodeSize)),
          );
          transaction.scrollIntoView();
          view.dispatch(transaction);
          return true;
        },
      },
    }),
  ],
});

export function insertInlineMathForEditing(editor: KoshBlockNoteEditor): void {
  editor.insertInlineContent([{ type: "inlineMath", props: { latex: "" } }], {
    updateSelection: true,
  });

  const position = editor.prosemirrorView.state.selection.from - 1;
  openInlineMathAtPosition(editor, position, 0);
}

function openInlineMathAtPosition(
  editor: KoshBlockNoteEditor,
  position: number,
  attempt: number,
): void {
  window.requestAnimationFrame(() => {
    const view = editor.prosemirrorView;
    if (view.isDestroyed) return;
    const nodeDom = view.nodeDOM(position);
    const element = nodeDom instanceof Element ? nodeDom : nodeDom?.parentElement;
    const trigger = element?.querySelector<HTMLElement>(".kosh-math-editor__trigger");
    if (trigger) {
      trigger.click();
      return;
    }
    if (attempt < 2) openInlineMathAtPosition(editor, position, attempt + 1);
  });
}

function hasCodeMark(marks: readonly { type: { name: string } }[]): boolean {
  return marks.some((mark) => mark.type.name === "code");
}

function isEscaped(textBefore: string, matchStart: number): boolean {
  let backslashes = 0;
  for (let index = matchStart - 1; index >= 0 && textBefore[index] === "\\"; index -= 1) {
    backslashes += 1;
  }
  return backslashes % 2 === 1;
}
