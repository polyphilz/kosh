import { defaultKeymap } from "@codemirror/commands";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState as CodeMirrorState, type Extension } from "@codemirror/state";
import {
  drawSelection,
  EditorView as CodeMirrorView,
  highlightSpecialChars,
  keymap as codeMirrorKeymap,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { exitCode } from "prosemirror-commands";
import { redo, undo } from "prosemirror-history";
import type { Node as ProseMirrorNode } from "prosemirror-model";
import { AllSelection, Selection, TextSelection } from "prosemirror-state";
import type { EditorView, NodeView } from "prosemirror-view";
import { codeLanguageDisplayName } from "./languages";
import { KOSH_WRITING_ASSISTANCE_ATTRIBUTES } from "./writingAssistance";

const koshCodeHighlightStyle = HighlightStyle.define([
  {
    tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment],
    color: "var(--code-syntax-comment)",
    fontStyle: "italic",
  },
  {
    tag: [
      tags.keyword,
      tags.controlKeyword,
      tags.definitionKeyword,
      tags.moduleKeyword,
      tags.modifier,
      tags.operatorKeyword,
    ],
    color: "var(--code-syntax-keyword)",
  },
  {
    tag: [tags.string, tags.regexp, tags.escape, tags.special(tags.string)],
    color: "var(--code-syntax-string)",
  },
  {
    tag: [tags.number, tags.bool, tags.null, tags.atom],
    color: "var(--code-syntax-number)",
  },
  {
    tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.labelName],
    color: "var(--code-syntax-function)",
  },
  {
    tag: [tags.typeName, tags.className, tags.namespace],
    color: "var(--code-syntax-type)",
  },
  {
    tag: [tags.propertyName, tags.attributeName],
    color: "var(--code-syntax-property)",
  },
  {
    tag: [tags.operator, tags.punctuation, tags.bracket],
    color: "var(--code-syntax-punctuation)",
  },
  {
    tag: [tags.meta, tags.annotation],
    color: "var(--code-syntax-comment)",
  },
  {
    tag: tags.invalid,
    color: "var(--code-syntax-invalid)",
    textDecoration: "underline wavy",
  },
]);

export function codeBlockNodeView(
  node: ProseMirrorNode,
  view: EditorView,
  getPos: () => number | undefined,
): NodeView {
  return new CodeBlockView(node, view, getPos);
}

class CodeBlockView implements NodeView {
  readonly dom: HTMLElement;
  private readonly cm: CodeMirrorView;
  private readonly editable = new Compartment();
  private readonly language = new Compartment();
  private readonly outerView: EditorView;
  private readonly getPos: () => number | undefined;
  private destroyed = false;
  private languageRequest = 0;
  private node: ProseMirrorNode;
  private updating = false;

  constructor(node: ProseMirrorNode, view: EditorView, getPos: () => number | undefined) {
    this.node = node;
    this.outerView = view;
    this.getPos = getPos;
    this.cm = new CodeMirrorView({
      doc: node.textContent,
      extensions: [
        highlightSpecialChars(),
        drawSelection(),
        syntaxHighlighting(koshCodeHighlightStyle, { fallback: true }),
        CodeMirrorView.editorAttributes.of({
          class: "kosh-code-block-editor",
        }),
        CodeMirrorView.contentAttributes.of({
          ...KOSH_WRITING_ASSISTANCE_ATTRIBUTES,
          tabindex: "-1",
        }),
        codeMirrorKeymap.of([
          ...this.navigationKeymap(),
          ...defaultKeymap.filter(
            (binding) => !["Escape", "Tab", "Shift-Tab"].includes(binding.key ?? ""),
          ),
        ]),
        this.language.of([]),
        this.editable.of(editableExtensions(view.editable)),
        CodeMirrorView.updateListener.of((update) => this.forwardUpdate(update)),
        CodeMirrorView.theme({
          "&": {
            color: "var(--code-block-ink)",
            backgroundColor: "var(--code-block-background)",
            border: "1px solid var(--code-block-border)",
            borderRadius: "8px",
          },
          "&.cm-focused": {
            outline: "none",
          },
          ".cm-content": {
            minHeight: "86px",
            padding: "12px 14px",
            caretColor: "var(--accent)",
            color: "var(--code-block-ink)",
            fontFamily: "var(--font-family-app)",
            fontSize: "13px",
          },
          ".cm-cursor, .cm-dropCursor": {
            borderLeftColor: "var(--accent)",
          },
          "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
            backgroundColor: "var(--code-block-selection)",
          },
          ".cm-gutters": { display: "none" },
          ".cm-scroller": { overflow: "auto" },
        }),
      ],
    });
    this.dom = this.cm.dom;
    this.setLanguageData(node.attrs.language);
    void this.configureLanguage(node.attrs.language);
  }

  update = (node: ProseMirrorNode): boolean => {
    if (node.type !== this.node.type) {
      return false;
    }
    const languageChanged = node.attrs.language !== this.node.attrs.language;
    this.node = node;
    const currentText = this.cm.state.doc.toString();
    if (node.textContent !== currentText) {
      this.updating = true;
      this.cm.dispatch({
        changes: {
          from: 0,
          insert: node.textContent,
          to: currentText.length,
        },
      });
      this.updating = false;
    }
    this.cm.dispatch({
      effects: this.editable.reconfigure(editableExtensions(this.outerView.editable)),
    });
    if (languageChanged) {
      this.setLanguageData(node.attrs.language);
      void this.configureLanguage(node.attrs.language);
    }
    return true;
  };

  setSelection = (anchor: number, head: number) => {
    this.cm.focus();
    this.updating = true;
    this.cm.dispatch({ selection: { anchor, head } });
    this.updating = false;
  };

  selectNode = () => this.cm.focus();
  stopEvent = () => true;
  ignoreMutation = () => true;

  destroy = () => {
    this.destroyed = true;
    this.cm.destroy();
  };

  private forwardUpdate(
    update: Parameters<NonNullable<Parameters<typeof CodeMirrorView.updateListener.of>[0]>>[0],
  ) {
    if (this.updating || !this.cm.hasFocus) {
      return;
    }
    const position = this.getPos();
    if (position === undefined) {
      return;
    }
    let offset = position + 1;
    const selection = update.state.selection.main;
    const selectionFrom = offset + selection.from;
    const selectionTo = offset + selection.to;
    const outerSelection = this.outerView.state.selection;
    if (
      !update.docChanged &&
      outerSelection.from === selectionFrom &&
      outerSelection.to === selectionTo
    ) {
      return;
    }

    const transaction = this.outerView.state.tr;
    update.changes.iterChanges((fromA, toA, fromB, toB, text) => {
      if (text.length) {
        transaction.replaceWith(
          offset + fromA,
          offset + toA,
          this.outerView.state.schema.text(text.toString()),
        );
      } else {
        transaction.delete(offset + fromA, offset + toA);
      }
      offset += toB - fromB - (toA - fromA);
    });
    transaction.setSelection(TextSelection.create(transaction.doc, selectionFrom, selectionTo));
    this.outerView.dispatch(transaction);
  }

  private navigationKeymap() {
    return [
      { key: "ArrowUp", run: () => this.maybeEscape("line", -1) },
      { key: "ArrowLeft", run: () => this.maybeEscape("char", -1) },
      { key: "ArrowDown", run: () => this.maybeEscape("line", 1) },
      { key: "ArrowRight", run: () => this.maybeEscape("char", 1) },
      {
        key: "Ctrl-Enter",
        run: () => {
          if (!exitCode(this.outerView.state, this.outerView.dispatch)) {
            return false;
          }
          this.outerView.focus();
          return true;
        },
      },
      { key: "Mod-a", run: () => this.selectAll() },
      { key: "Alt-F10", run: () => this.focusToolbar() },
      {
        key: "Mod-z",
        run: () => undo(this.outerView.state, this.outerView.dispatch),
      },
      {
        key: "Mod-Shift-z",
        run: () => redo(this.outerView.state, this.outerView.dispatch),
      },
      {
        key: "Mod-y",
        run: () => redo(this.outerView.state, this.outerView.dispatch),
      },
    ];
  }

  private selectAll(): boolean {
    const selection = this.cm.state.selection.main;
    const documentLength = this.cm.state.doc.length;
    const allCodeSelected = selection.from === 0 && selection.to === documentLength;
    if (documentLength > 0 && !allCodeSelected) {
      this.cm.dispatch({ selection: { anchor: 0, head: documentLength } });
      return true;
    }
    this.outerView.dispatch(
      this.outerView.state.tr.setSelection(new AllSelection(this.outerView.state.doc)),
    );
    this.outerView.focus();
    return true;
  }

  private focusToolbar(): boolean {
    const toolbar = this.outerView.dom
      .closest(".kosh-rich-text-editor")
      ?.querySelector<HTMLElement>(".kosh-rich-text-toolbar");
    if (!toolbar) {
      return false;
    }
    toolbar.focus();
    return true;
  }

  private maybeEscape(unit: "char" | "line", direction: -1 | 1): boolean {
    const selection = this.cm.state.selection.main;
    if (!selection.empty) {
      return false;
    }
    const range = unit === "line" ? this.cm.state.doc.lineAt(selection.head) : selection;
    if (direction < 0 ? range.from > 0 : range.to < this.cm.state.doc.length) {
      return false;
    }
    const position = this.getPos();
    if (position === undefined) {
      return false;
    }
    const target = position + (direction < 0 ? 0 : this.node.nodeSize);
    const outerSelection = Selection.near(this.outerView.state.doc.resolve(target), direction);
    this.outerView.dispatch(this.outerView.state.tr.setSelection(outerSelection).scrollIntoView());
    this.outerView.focus();
    return true;
  }

  private async configureLanguage(language: string | null) {
    const request = ++this.languageRequest;
    const extension = await loadCodeLanguage(language);
    if (this.destroyed || request !== this.languageRequest) {
      return;
    }
    this.cm.dispatch({
      effects: this.language.reconfigure(extension ? [extension] : []),
    });
  }

  private setLanguageData(language: string | null) {
    this.dom.dataset.language = language ?? "";
    this.dom.dataset.languageLabel = language ? codeLanguageDisplayName(language) : "";
  }
}

function editableExtensions(editable: boolean): Extension[] {
  return [CodeMirrorState.readOnly.of(!editable), CodeMirrorView.editable.of(editable)];
}

async function loadCodeLanguage(language: string | null): Promise<Extension | null> {
  switch (language) {
    case "c":
    case "cpp":
      return (await import("@codemirror/lang-cpp")).cpp();
    case "css":
      return (await import("@codemirror/lang-css")).css();
    case "go":
      return (await import("@codemirror/lang-go")).go();
    case "html":
      return (await import("@codemirror/lang-html")).html();
    case "xml":
      return (await import("@codemirror/lang-xml")).xml();
    case "java":
      return (await import("@codemirror/lang-java")).java();
    case "javascript":
      return (await import("@codemirror/lang-javascript")).javascript();
    case "jsx":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
    case "typescript":
      return (await import("@codemirror/lang-javascript")).javascript({
        typescript: true,
      });
    case "tsx":
      return (await import("@codemirror/lang-javascript")).javascript({
        jsx: true,
        typescript: true,
      });
    case "json":
      return (await import("@codemirror/lang-json")).json();
    case "markdown":
      return (await import("@codemirror/lang-markdown")).markdown();
    case "python":
      return (await import("@codemirror/lang-python")).python();
    case "rust":
      return (await import("@codemirror/lang-rust")).rust();
    case "sql":
      return (await import("@codemirror/lang-sql")).sql();
    case "yaml":
      return (await import("@codemirror/lang-yaml")).yaml();
    default:
      return null;
  }
}
