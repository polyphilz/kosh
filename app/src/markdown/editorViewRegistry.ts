import type { EditorView } from "prosemirror-view";

const editorViews = new WeakMap<HTMLElement, EditorView>();

export function registerRichTextEditorView(view: EditorView): void {
  editorViews.set(view.dom, view);
}

export function unregisterRichTextEditorView(view: EditorView): void {
  editorViews.delete(view.dom);
}

export function richTextEditorViewFromDOM(element: HTMLElement): EditorView | null {
  return editorViews.get(element) ?? null;
}
