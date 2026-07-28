import {
  baseKeymap,
  chainCommands,
  createParagraphNear,
  liftEmptyBlock,
  newlineInCode,
  setBlockType,
  splitBlock,
  toggleMark,
  wrapIn,
} from "prosemirror-commands";
import { dropCursor } from "prosemirror-dropcursor";
import { gapCursor } from "prosemirror-gapcursor";
import { history, redo, undo } from "prosemirror-history";
import {
  InputRule,
  inputRules,
  textblockTypeInputRule,
  undoInputRule,
  wrappingInputRule,
} from "prosemirror-inputrules";
import { keymap } from "prosemirror-keymap";
import type { Node as ProseMirrorNode } from "prosemirror-model";
import { liftListItem, sinkListItem, splitListItem, wrapInList } from "prosemirror-schema-list";
import { EditorState, NodeSelection, Selection, type Command } from "prosemirror-state";
import { tableEditing } from "prosemirror-tables";
import { insertPoint, liftTarget } from "prosemirror-transform";
import {
  EditorView,
  type DirectEditorProps,
  type NodeView,
  type NodeViewConstructor,
} from "prosemirror-view";
import katex from "katex";
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { Select, type SelectOption } from "../components/Select";
import { koshEditorSchema } from "./editorSchema";
import { KOSH_EDITOR_EDITABLE_EVENT } from "./editorEvents";
import { registerRichTextEditorView, unregisterRichTextEditorView } from "./editorViewRegistry";
import { codeLanguageDefinitions, codeLanguageDisplayName } from "./languages";
import { parseKoshMarkdown, serializeKoshMarkdown } from "./markdownConversion";
import { externalHttpUrl } from "./urlPolicy";
import { KOSH_WRITING_ASSISTANCE_ATTRIBUTES } from "./writingAssistance";

export interface RichTextEditorHandle {
  focus: () => void;
}

interface RichTextEditorProps {
  ariaLabel: string;
  disabled?: boolean;
  onChange: (value: string) => void;
  placeholder?: string;
  value: string;
}

interface MathDialogState {
  display: boolean;
  formula: string;
  position?: number;
}

interface LinkDialogState {
  href: string;
}

const externalValueUpdate = "kosh-external-value-update";
let codeBlockNodeViewPromise: Promise<NodeViewConstructor> | null = null;

export const RichTextEditor = forwardRef<RichTextEditorHandle, RichTextEditorProps>(
  function RichTextEditor({ ariaLabel, disabled = false, onChange, placeholder, value }, ref) {
    const hostRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    const onChangeRef = useRef(onChange);
    const disabledRef = useRef(disabled);
    const openMathDialogRef = useRef(
      (_display: boolean, _formula: string, _position?: number) => undefined,
    );
    const openLinkDialogRef = useRef((_view: EditorView) => undefined);
    const initialValueRef = useRef(value);
    const initialAriaLabelRef = useRef(ariaLabel);
    const initialPlaceholderRef = useRef(placeholder);
    const [, renderToolbar] = useReducer((count) => count + 1, 0);
    const [mathDialog, setMathDialog] = useState<MathDialogState | null>(null);
    const [linkDialog, setLinkDialog] = useState<LinkDialogState | null>(null);

    onChangeRef.current = onChange;
    disabledRef.current = disabled;
    openMathDialogRef.current = (display, formula, position) => {
      if (disabledRef.current) {
        return;
      }
      setLinkDialog(null);
      setMathDialog({ display, formula, position });
    };
    openLinkDialogRef.current = (editorView) => {
      setMathDialog(null);
      setLinkDialog({ href: activeLinkHref(editorView) ?? "https://" });
    };

    useImperativeHandle(ref, () => ({ focus: () => viewRef.current?.focus() }), []);

    useLayoutEffect(() => {
      const host = hostRef.current;
      if (!host) {
        return;
      }

      const state = EditorState.create({
        doc: parseKoshMarkdown(initialValueRef.current, koshEditorSchema),
        plugins: [
          history(),
          editorInputRules(),
          keymap(editorKeyBindings((editorView) => openLinkDialogRef.current(editorView))),
          keymap(baseKeymap),
          dropCursor(),
          gapCursor(),
          tableEditing(),
        ],
        schema: koshEditorSchema,
      });

      const props: DirectEditorProps = {
        attributes: {
          ...KOSH_WRITING_ASSISTANCE_ATTRIBUTES,
          "aria-label": initialAriaLabelRef.current,
          "aria-multiline": "true",
          class: "kosh-rich-text-content",
          role: "textbox",
        },
        dispatchTransaction(transaction) {
          const view = viewRef.current;
          if (!view) {
            return;
          }
          const nextState = view.state.apply(transaction);
          view.updateState(nextState);
          updateEmptyState(view);
          void installCodeBlockNodeView(view);
          renderToolbar();
          if (transaction.docChanged && transaction.getMeta(externalValueUpdate) !== true) {
            onChangeRef.current(serializeKoshMarkdown(nextState.doc));
          }
        },
        editable: () => !disabledRef.current,
        nodeViews: {
          math_display: (node, editorView, getPos) =>
            mathNodeView(node, editorView, getPos, openMathDialogRef.current),
          math_inline: (node, editorView, getPos) =>
            mathNodeView(node, editorView, getPos, openMathDialogRef.current),
        },
        state,
      };

      const view = new EditorView(host, props);
      viewRef.current = view;
      registerRichTextEditorView(view);
      view.dom.dataset.placeholder = initialPlaceholderRef.current ?? "";
      updateEmptyState(view);
      void installCodeBlockNodeView(view);
      renderToolbar();

      return () => {
        viewRef.current = null;
        unregisterRichTextEditorView(view);
        view.destroy();
      };
    }, []);

    useEffect(() => {
      const view = viewRef.current;
      if (!view) {
        return;
      }
      view.setProps({ editable: () => !disabled });
      view.dom.setAttribute("aria-disabled", String(disabled));
      view.dom.dispatchEvent(new Event(KOSH_EDITOR_EDITABLE_EVENT));
      if (disabled) {
        setMathDialog(null);
        setLinkDialog(null);
      }
      renderToolbar();
    }, [disabled]);

    useEffect(() => {
      const view = viewRef.current;
      if (!view) {
        return;
      }
      const current = serializeKoshMarkdown(view.state.doc);
      if (current === value) {
        return;
      }
      const replacement = parseKoshMarkdown(value, koshEditorSchema);
      if (view.state.doc.eq(replacement)) {
        return;
      }
      const transaction = view.state.tr
        .replaceWith(0, view.state.doc.content.size, replacement.content)
        .setMeta(externalValueUpdate, true)
        .setMeta("addToHistory", false);
      view.dispatch(transaction);
    }, [value]);

    const view = viewRef.current;
    return (
      <div
        aria-disabled={disabled || undefined}
        className="kosh-rich-text-editor"
        data-testid={`${ariaLabel.toLowerCase().replace(/\s+/gu, "-")}-editor`}
      >
        <EditorToolbar
          ariaLabel={ariaLabel}
          disabled={disabled}
          onLink={() => view && openLinkDialogRef.current(view)}
          onMath={(display) => {
            setLinkDialog(null);
            setMathDialog({ display, formula: "" });
          }}
          view={view}
        />
        <div className="kosh-rich-text-editor__surface" ref={hostRef} />
        {mathDialog && view && (
          <FormulaDialog
            dialog={mathDialog}
            onCancel={() => {
              setMathDialog(null);
              view.focus();
            }}
            onConfirm={(formula) => {
              commitMath(view, mathDialog, formula);
              setMathDialog(null);
            }}
          />
        )}
        {linkDialog && view && (
          <LinkDialog
            dialog={linkDialog}
            onCancel={() => {
              setLinkDialog(null);
              view.focus();
            }}
            onConfirm={(href) => {
              applyLink(view, href);
              setLinkDialog(null);
            }}
          />
        )}
      </div>
    );
  },
);

function editorKeyBindings(openLink: (view: EditorView) => void): Record<string, Command> {
  const listItem = koshEditorSchema.nodes.list_item!;
  const hardBreak = insertHardBreak();
  return {
    "Alt-F10": focusEditorToolbar,
    Backspace: undoInputRule,
    "Mod-[": liftListItem(listItem),
    "Mod-]": sinkListItem(listItem),
    "Mod-b": toggleMark(koshEditorSchema.marks.strong!),
    "Mod-e": toggleMark(koshEditorSchema.marks.code!),
    "Mod-i": toggleMark(koshEditorSchema.marks.em!),
    "Mod-k": (_state, _dispatch, view) => {
      if (!view) {
        return false;
      }
      openLink(view);
      return true;
    },
    "Mod-Shift-x": toggleMark(koshEditorSchema.marks.strike!),
    "Mod-z": undo,
    "Mod-y": redo,
    "Mod-Shift-z": redo,
    Enter: chainCommands(
      splitListItem(listItem),
      newlineInCode,
      createParagraphNear,
      liftEmptyBlock,
      splitBlock,
    ),
    "Shift-Enter": hardBreak,
    ArrowDown: arrowIntoCodeBlock("down"),
    ArrowLeft: arrowIntoCodeBlock("left"),
    ArrowRight: arrowIntoCodeBlock("right"),
    ArrowUp: arrowIntoCodeBlock("up"),
  };
}

const focusEditorToolbar: Command = (_state, _dispatch, view) => {
  const toolbar = view?.dom
    .closest(".kosh-rich-text-editor")
    ?.querySelector<HTMLElement>(".kosh-rich-text-toolbar");
  if (!toolbar) {
    return false;
  }
  toolbar.focus();
  return true;
};

function editorInputRules() {
  const bulletList = koshEditorSchema.nodes.bullet_list!;
  const codeBlock = koshEditorSchema.nodes.code_block!;
  const orderedList = koshEditorSchema.nodes.ordered_list!;
  const heading = koshEditorSchema.nodes.heading!;
  const blockquote = koshEditorSchema.nodes.blockquote!;
  return inputRules({
    rules: [
      textblockTypeInputRule(/^```$/u, codeBlock, { language: null }),
      textblockTypeInputRule(/^(#{1,6})\s$/u, heading, (match) => ({
        level: match[1]!.length,
      })),
      inlineCodeInputRule(),
      wrappingInputRule(/^\s*>\s$/u, blockquote),
      wrappingInputRule(/^[-*•]\s$/u, bulletList),
      wrappingInputRule(
        /^(\d+)\.\s$/u,
        orderedList,
        (match) => ({ order: Number(match[1]) }),
        (match, node) => node.childCount + Number(node.attrs.order) === Number(match[1]),
      ),
    ],
  });
}

function inlineCodeInputRule(): InputRule {
  return new InputRule(
    /`([^`\n]+)`$/u,
    (state, _match, start, end) => {
      const code = state.schema.marks.code;
      if (!code) {
        return null;
      }
      return state.tr.delete(start, start + 1).addMark(start, end - 1, code.create());
    },
    { inCodeMark: false },
  );
}

function arrowIntoCodeBlock(direction: "down" | "left" | "right" | "up"): Command {
  return (state, dispatch, view) => {
    if (!view || !state.selection.empty || !view.endOfTextblock(direction)) {
      return false;
    }
    const side = direction === "left" || direction === "up" ? -1 : 1;
    const { $head } = state.selection;
    if ($head.depth === 0) {
      return false;
    }
    const next = Selection.near(state.doc.resolve(side > 0 ? $head.after() : $head.before()), side);
    if (next.$head.parent.type.name !== "code_block") {
      return false;
    }
    dispatch?.(state.tr.setSelection(next));
    return true;
  };
}

function insertHardBreak(): Command {
  return (state, dispatch) => {
    const hardBreak = state.schema.nodes.hard_break;
    if (!hardBreak) {
      return false;
    }
    if (dispatch) {
      dispatch(state.tr.replaceSelectionWith(hardBreak.create()).scrollIntoView());
    }
    return true;
  };
}

function EditorToolbar({
  ariaLabel,
  disabled,
  onLink,
  onMath,
  view,
}: {
  ariaLabel: string;
  disabled: boolean;
  onLink: () => void;
  onMath: (display: boolean) => void;
  view: EditorView | null;
}) {
  const commandButton = (
    label: string,
    shortLabel: string,
    command: Command,
    active = false,
    shortcut?: string,
  ) => (
    <ToolbarButton
      active={active}
      disabled={disabled || !view || !command(view.state)}
      key={label}
      label={label}
      onPress={() => view && runCommand(view, command)}
      shortcut={shortcut}
    >
      {shortLabel}
    </ToolbarButton>
  );

  return (
    <div
      aria-label={`${ariaLabel} formatting`}
      aria-keyshortcuts="Alt+F10"
      className="kosh-rich-text-toolbar"
      onKeyDown={handleToolbarKeys}
      role="toolbar"
      tabIndex={-1}
    >
      <BlockTypeControl disabled={disabled} view={view} />
      {commandButton(
        "Bold",
        "B",
        toggleMark(koshEditorSchema.marks.strong!),
        markIsActive(view, "strong"),
        "⌘B",
      )}
      {commandButton(
        "Italic",
        "I",
        toggleMark(koshEditorSchema.marks.em!),
        markIsActive(view, "em"),
        "⌘I",
      )}
      {commandButton(
        "Strikethrough",
        "S",
        toggleMark(koshEditorSchema.marks.strike!),
        markIsActive(view, "strike"),
      )}
      {commandButton(
        "Inline code",
        "</>",
        toggleMark(koshEditorSchema.marks.code!),
        markIsActive(view, "code"),
      )}
      <ToolbarButton
        active={markIsActive(view, "link")}
        disabled={disabled || !view}
        label="Link"
        onPress={onLink}
        shortcut="⌘K"
      >
        Link
      </ToolbarButton>
      <span aria-hidden="true" className="kosh-rich-text-toolbar__divider" />
      {commandButton(
        "Bulleted list",
        "• List",
        toggleList("bullet_list"),
        blockIsActive(view, "bullet_list"),
      )}
      {commandButton(
        "Numbered list",
        "1. List",
        toggleList("ordered_list"),
        blockIsActive(view, "ordered_list"),
      )}
      {commandButton(
        "Block quote",
        "Quote",
        toggleBlock("blockquote"),
        blockIsActive(view, "blockquote"),
      )}
      {commandButton(
        "Code block",
        "Code",
        toggleTextBlock("code_block"),
        blockIsActive(view, "code_block"),
      )}
      <CodeLanguageControl disabled={disabled} view={view} />
      {commandButton("Horizontal rule", "—", insertHorizontalRule())}
      {commandButton("Table", "Table", insertTable())}
      <span aria-hidden="true" className="kosh-rich-text-toolbar__divider" />
      <ToolbarButton disabled={disabled || !view} label="Inline math" onPress={() => onMath(false)}>
        ƒx
      </ToolbarButton>
      <ToolbarButton disabled={disabled || !view} label="Display math" onPress={() => onMath(true)}>
        ∑
      </ToolbarButton>
      {commandButton("Undo", "↶", undo, false, "⌘Z")}
      {commandButton("Redo", "↷", redo, false, "⇧⌘Z")}
    </div>
  );
}

const blockTypeOptions = [
  { label: "Paragraph", value: "paragraph" },
  { label: "Heading 1", value: "heading-1" },
  { label: "Heading 2", value: "heading-2" },
  { label: "Heading 3", value: "heading-3" },
] as const satisfies readonly SelectOption<string>[];

function BlockTypeControl({ disabled, view }: { disabled: boolean; view: EditorView | null }) {
  const value = activeBlockType(view);
  return (
    <Select
      aria-label="Text style"
      className="kosh-rich-text-toolbar__select"
      disabled={disabled || !view || value === null}
      onValueChange={(next) => {
        if (view) {
          setTextStyle(view, next);
        }
      }}
      options={blockTypeOptions}
      tabIndex={-1}
      value={value ?? "paragraph"}
    />
  );
}

function CodeLanguageControl({ disabled, view }: { disabled: boolean; view: EditorView | null }) {
  const language = activeCodeLanguage(view);
  if (language === undefined) {
    return null;
  }
  const known = codeLanguageDefinitions.some((definition) => definition.canonical === language);
  const options: SelectOption<string>[] = [
    { label: "Plain code", value: "" },
    ...(language && !known ? [{ label: codeLanguageDisplayName(language), value: language }] : []),
    ...codeLanguageDefinitions.map((definition) => ({
      label: codeLanguageDisplayName(definition.canonical),
      value: definition.canonical,
    })),
  ];
  return (
    <Select
      aria-label="Code language"
      className="kosh-rich-text-toolbar__select"
      disabled={disabled}
      onValueChange={(next) => {
        if (view) {
          setCodeLanguage(view, next || null);
        }
      }}
      options={options}
      tabIndex={-1}
      value={language ?? ""}
    />
  );
}

function ToolbarButton({
  active = false,
  children,
  disabled,
  label,
  onPress,
  shortcut,
}: {
  active?: boolean;
  children: string;
  disabled: boolean;
  label: string;
  onPress: () => void;
  shortcut?: string;
}) {
  const handleMouseDown = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    onPress();
  };
  return (
    <Button
      aria-keyshortcuts={shortcut}
      aria-label={label}
      aria-pressed={active}
      className={
        active
          ? "kosh-rich-text-toolbar__button kosh-rich-text-toolbar__button--active"
          : "kosh-rich-text-toolbar__button"
      }
      disabled={disabled}
      onMouseDown={handleMouseDown}
      size="compact"
      tabIndex={-1}
      title={shortcut ? `${label} (${shortcut})` : label}
      variant="ghost"
    >
      {children}
    </Button>
  );
}

function handleToolbarKeys(event: KeyboardEvent<HTMLDivElement>) {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
    return;
  }
  const controls = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>(
      "button:not(:disabled), select:not(:disabled)",
    ),
  );
  if (!controls.length) {
    return;
  }
  event.preventDefault();
  const current = controls.indexOf(document.activeElement as HTMLElement);
  const next =
    event.key === "Home"
      ? 0
      : event.key === "End"
        ? controls.length - 1
        : event.key === "ArrowRight"
          ? (current + 1 + controls.length) % controls.length
          : (current - 1 + controls.length) % controls.length;
  controls[next]?.focus();
}

function runCommand(view: EditorView, command: Command): boolean {
  const applied = command(view.state, view.dispatch, view);
  if (applied) {
    view.focus();
  }
  return applied;
}

function markIsActive(view: EditorView | null, markName: string): boolean {
  if (!view) {
    return false;
  }
  const type = view.state.schema.marks[markName];
  if (!type) {
    return false;
  }
  const { empty, from, $from, to } = view.state.selection;
  return empty
    ? Boolean(type.isInSet(view.state.storedMarks ?? $from.marks()))
    : view.state.doc.rangeHasMark(from, to, type);
}

function blockIsActive(view: EditorView | null, nodeName: string): boolean {
  if (!view) {
    return false;
  }
  const { $from } = view.state.selection;
  for (let depth = $from.depth; depth >= 0; depth -= 1) {
    if ($from.node(depth).type.name === nodeName) {
      return true;
    }
  }
  return false;
}

function activeBlockType(view: EditorView | null): string | null {
  if (!view) {
    return "paragraph";
  }
  const { $from } = view.state.selection;
  if (!$from.parent.isTextblock || ["code_block"].includes($from.parent.type.name)) {
    return null;
  }
  return $from.parent.type.name === "heading"
    ? `heading-${String($from.parent.attrs.level)}`
    : "paragraph";
}

function setTextStyle(view: EditorView, value: string): void {
  const match = /^heading-([1-3])$/u.exec(value);
  const command = match
    ? setBlockType(view.state.schema.nodes.heading!, { level: Number(match[1]) })
    : setBlockType(view.state.schema.nodes.paragraph!);
  runCommand(view, command);
}

function activeCodeLanguage(view: EditorView | null): string | null | undefined {
  if (!view) {
    return undefined;
  }
  const { $from } = view.state.selection;
  for (let depth = $from.depth; depth >= 0; depth -= 1) {
    const node = $from.node(depth);
    if (node.type.name === "code_block") {
      return node.attrs.language ?? null;
    }
  }
  return undefined;
}

function setCodeLanguage(view: EditorView, language: string | null): void {
  const { $from } = view.state.selection;
  for (let depth = $from.depth; depth >= 1; depth -= 1) {
    const node = $from.node(depth);
    if (node.type.name === "code_block") {
      view.dispatch(
        view.state.tr.setNodeMarkup($from.before(depth), undefined, {
          ...node.attrs,
          language,
        }),
      );
      view.focus();
      return;
    }
  }
}

function toggleList(nodeName: "bullet_list" | "ordered_list"): Command {
  return (state, dispatch) => {
    const list = state.schema.nodes[nodeName]!;
    const item = state.schema.nodes.list_item!;
    return ancestorIsActive(state, nodeName)
      ? liftListItem(item)(state, dispatch)
      : wrapInList(list)(state, dispatch);
  };
}

function toggleBlock(nodeName: "blockquote"): Command {
  return (state, dispatch) => {
    const type = state.schema.nodes[nodeName]!;
    if (ancestorIsActive(state, nodeName)) {
      const { $from, $to } = state.selection;
      const range = $from.blockRange($to);
      const target = range ? liftTarget(range) : null;
      if (!range || target === null) {
        return false;
      }
      dispatch?.(state.tr.lift(range, target).scrollIntoView());
      return true;
    }
    return wrapIn(type)(state, dispatch);
  };
}

function toggleTextBlock(nodeName: "code_block"): Command {
  return (state, dispatch) =>
    setBlockType(
      ancestorIsActive(state, nodeName)
        ? state.schema.nodes.paragraph!
        : state.schema.nodes[nodeName]!,
    )(state, dispatch);
}

function ancestorIsActive(state: EditorState, nodeName: string): boolean {
  const { $from } = state.selection;
  for (let depth = $from.depth; depth >= 0; depth -= 1) {
    if ($from.node(depth).type.name === nodeName) {
      return true;
    }
  }
  return false;
}

function insertHorizontalRule(): Command {
  return (state, dispatch) => {
    const type = state.schema.nodes.horizontal_rule;
    if (!type) {
      return false;
    }
    const position = insertPoint(state.doc, state.selection.to, type);
    if (position === null) {
      return false;
    }
    dispatch?.(state.tr.insert(position, type.create()).scrollIntoView());
    return true;
  };
}

function insertTable(): Command {
  return (state, dispatch) => {
    const table = state.schema.nodes.table;
    const row = state.schema.nodes.table_row;
    const header = state.schema.nodes.table_header;
    const cell = state.schema.nodes.table_cell;
    const paragraph = state.schema.nodes.paragraph;
    if (!table || !row || !header || !cell || !paragraph) {
      return false;
    }
    const position = insertPoint(state.doc, state.selection.to, table);
    if (position === null) {
      return false;
    }
    const empty = paragraph.create();
    const node = table.create(null, [
      row.create(null, [header.create(null, empty), header.create(null, empty)]),
      row.create(null, [cell.create(null, empty), cell.create(null, empty)]),
    ]);
    dispatch?.(state.tr.insert(position, node).scrollIntoView());
    return true;
  };
}

function activeLinkHref(view: EditorView): string | null {
  const current = view.state.schema.marks.link!.isInSet(
    view.state.storedMarks ?? view.state.selection.$from.marks(),
  );
  return typeof current?.attrs.href === "string" ? current.attrs.href : null;
}

function applyLink(view: EditorView, href: string | null): void {
  const link = view.state.schema.marks.link!;
  const { from, to, empty } = view.state.selection;
  if (empty) {
    const range = activeMarkRange(view, "link");
    if (range) {
      let transaction = view.state.tr.removeMark(range.from, range.to, link);
      if (href) {
        transaction = transaction.addMark(range.from, range.to, link.create({ href, title: null }));
      }
      view.dispatch(transaction.scrollIntoView());
      view.focus();
    } else if (href) {
      runCommand(view, toggleMark(link, { href, title: null }));
    } else {
      view.focus();
    }
    return;
  }
  let transaction = view.state.tr.removeMark(from, to, link);
  if (href) {
    transaction = transaction.addMark(from, to, link.create({ href, title: null }));
  }
  view.dispatch(transaction.scrollIntoView());
  view.focus();
}

function activeMarkRange(view: EditorView, markName: string): { from: number; to: number } | null {
  const { $from } = view.state.selection;
  const markType = view.state.schema.marks[markName];
  const active = markType?.isInSet($from.marks());
  if (!active) {
    return null;
  }

  const siblings: Array<{
    from: number;
    node: ProseMirrorNode;
    to: number;
  }> = [];
  $from.parent.forEach((node, offset) => {
    siblings.push({
      from: $from.start() + offset,
      node,
      to: $from.start() + offset + node.nodeSize,
    });
  });
  const current = siblings.findIndex(
    ({ from, node, to }) =>
      from <= $from.pos && $from.pos <= to && Boolean(active.isInSet(node.marks)),
  );
  if (current < 0) {
    return null;
  }

  const hasSameMark = (index: number) => {
    const candidate = active.type.isInSet(siblings[index]!.node.marks);
    return candidate ? active.eq(candidate) : false;
  };
  let start = current;
  let end = current;
  while (start > 0 && hasSameMark(start - 1)) {
    start -= 1;
  }
  while (end + 1 < siblings.length && hasSameMark(end + 1)) {
    end += 1;
  }
  return {
    from: siblings[start]!.from,
    to: siblings[end]!.to,
  };
}

function commitMath(view: EditorView, dialog: MathDialogState, formula: string): void {
  if (dialog.position !== undefined) {
    const node = view.state.doc.nodeAt(dialog.position);
    if (node && ["math_inline", "math_display"].includes(node.type.name)) {
      view.dispatch(view.state.tr.setNodeMarkup(dialog.position, undefined, { formula }));
    }
    view.focus();
    return;
  }
  const type = dialog.display
    ? view.state.schema.nodes.math_display!
    : view.state.schema.nodes.math_inline!;
  let transaction = view.state.tr;
  if (dialog.display) {
    const position =
      insertPoint(view.state.doc, view.state.selection.to, type) ??
      insertPoint(view.state.doc, view.state.selection.from, type);
    if (position === null) {
      view.focus();
      return;
    }
    transaction = transaction.insert(position, type.create({ formula }));
    transaction = transaction.setSelection(NodeSelection.create(transaction.doc, position));
  } else {
    transaction = transaction.replaceSelectionWith(type.create({ formula }));
  }
  view.dispatch(transaction.scrollIntoView());
  view.focus();
}

function FormulaDialog({
  dialog,
  onCancel,
  onConfirm,
}: {
  dialog: MathDialogState;
  onCancel: () => void;
  onConfirm: (formula: string) => void;
}) {
  const [formula, setFormula] = useState(dialog.formula);
  const inputRef = useRef<HTMLInputElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  useEffect(() => {
    if (!previewRef.current) {
      return;
    }
    katex.render(formula || "\\square", previewRef.current, {
      displayMode: dialog.display,
      maxExpand: 1_000,
      maxSize: 20,
      output: "htmlAndMathml",
      strict: "warn",
      throwOnError: false,
      trust: false,
    });
  }, [dialog.display, formula]);

  const insertSnippet = (snippet: string, cursorOffset = snippet.length) => {
    const input = inputRef.current;
    if (!input) {
      return;
    }
    const start = input.selectionStart ?? formula.length;
    const end = input.selectionEnd ?? start;
    setFormula(formula.slice(0, start) + snippet + formula.slice(end));
    requestAnimationFrame(() => {
      input.focus();
      const cursor = start + cursorOffset;
      input.setSelectionRange(cursor, cursor);
    });
  };

  return (
    <form
      aria-label={dialog.display ? "Display math editor" : "Inline math editor"}
      className="kosh-editor-dialog"
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Escape") {
          event.preventDefault();
          onCancel();
        }
      }}
      onSubmit={(event) => {
        event.preventDefault();
        if (formula.trim()) {
          onConfirm(formula);
        }
      }}
      role="dialog"
    >
      <div className="kosh-editor-dialog__heading">
        <strong>{dialog.display ? "Display equation" : "Inline equation"}</strong>
        <span>LaTeX-style formula</span>
      </div>
      <div aria-live="polite" className="kosh-editor-dialog__preview" ref={previewRef} />
      <Input
        {...KOSH_WRITING_ASSISTANCE_ATTRIBUTES}
        aria-label="Formula"
        onChange={(event) => setFormula(event.target.value)}
        ref={inputRef}
        type="text"
        value={formula}
      />
      <div aria-label="Math symbols" className="kosh-editor-dialog__symbols" role="toolbar">
        {[
          ["π", "\\pi", 3],
          ["√", "\\sqrt{}", 6],
          ["x²", "^{}", 2],
          ["a⁄b", "\\frac{}{}", 6],
          ["∑", "\\sum_{}^{}", 6],
          ["∞", "\\infty", 6],
        ].map(([label, snippet, offset]) => (
          <Button
            key={label}
            onClick={() => insertSnippet(String(snippet), Number(offset))}
            size="compact"
            variant="ghost"
          >
            {label}
          </Button>
        ))}
      </div>
      <div className="kosh-editor-dialog__actions">
        <Button onClick={onCancel} variant="ghost">
          Cancel
        </Button>
        <Button disabled={!formula.trim()} type="submit" variant="primary">
          Apply
        </Button>
      </div>
    </form>
  );
}

function LinkDialog({
  dialog,
  onCancel,
  onConfirm,
}: {
  dialog: LinkDialogState;
  onCancel: () => void;
  onConfirm: (href: string | null) => void;
}) {
  const [href, setHref] = useState(dialog.href);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  return (
    <form
      aria-label="Link editor"
      className="kosh-editor-dialog"
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Escape") {
          event.preventDefault();
          onCancel();
        }
      }}
      onSubmit={(event) => {
        event.preventDefault();
        const entered = href.trim();
        if (!entered) {
          onConfirm(null);
          return;
        }
        const safeHref = externalHttpUrl(entered);
        if (!safeHref) {
          setError("Links must use http:// or https://.");
          return;
        }
        onConfirm(safeHref);
      }}
      role="dialog"
    >
      <div className="kosh-editor-dialog__heading">
        <strong>Link</strong>
        <span>Absolute HTTP URL</span>
      </div>
      <Input
        {...KOSH_WRITING_ASSISTANCE_ATTRIBUTES}
        aria-label="Link URL"
        onChange={(event) => {
          setHref(event.target.value);
          setError(null);
        }}
        ref={inputRef}
        type="url"
        value={href}
      />
      {error && <span role="alert">{error}</span>}
      <div className="kosh-editor-dialog__actions">
        <Button onClick={onCancel} variant="ghost">
          Cancel
        </Button>
        <Button type="submit" variant="primary">
          Apply
        </Button>
      </div>
    </form>
  );
}

function mathNodeView(
  node: ProseMirrorNode,
  _view: EditorView,
  getPos: () => number | undefined,
  openEditor: (display: boolean, formula: string, position?: number) => void,
): NodeView {
  return new MathNodeView(node, getPos, openEditor);
}

class MathNodeView implements NodeView {
  dom: HTMLElement;
  private node: ProseMirrorNode;
  private readonly getPos: () => number | undefined;
  private readonly openEditor: (display: boolean, formula: string, position?: number) => void;

  constructor(
    node: ProseMirrorNode,
    getPos: () => number | undefined,
    openEditor: (display: boolean, formula: string, position?: number) => void,
  ) {
    this.node = node;
    this.getPos = getPos;
    this.openEditor = openEditor;
    this.dom = document.createElement(node.type.name === "math_inline" ? "span" : "div");
    this.dom.className = `kosh-math-node ${
      node.type.name === "math_inline" ? "kosh-math-inline" : "kosh-math-display"
    }`;
    this.dom.contentEditable = "false";
    this.dom.tabIndex = 0;
    this.dom.addEventListener("dblclick", this.edit);
    this.dom.addEventListener("keydown", this.handleKeyDown);
    this.render();
  }

  update = (node: ProseMirrorNode): boolean => {
    if (node.type !== this.node.type) {
      return false;
    }
    this.node = node;
    this.render();
    return true;
  };

  selectNode = () => this.dom.classList.add("kosh-math-node--selected");
  deselectNode = () => this.dom.classList.remove("kosh-math-node--selected");

  destroy = () => {
    this.dom.removeEventListener("dblclick", this.edit);
    this.dom.removeEventListener("keydown", this.handleKeyDown);
  };

  stopEvent = (event: Event) => event.type === "dblclick" || event.type === "keydown";

  private handleKeyDown = (event: globalThis.KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      this.edit();
    }
  };

  private edit = () => {
    const position = this.getPos();
    if (position === undefined) {
      return;
    }
    this.openEditor(this.node.type.name === "math_display", this.node.attrs.formula, position);
  };

  private render() {
    const formula = this.node.attrs.formula as string;
    this.dom.dataset.formula = formula;
    this.dom.setAttribute("aria-label", `Math: ${formula || "empty formula"}`);
    katex.render(formula || "\\square", this.dom, {
      displayMode: this.node.type.name === "math_display",
      maxExpand: 1_000,
      maxSize: 20,
      output: "htmlAndMathml",
      strict: "warn",
      throwOnError: false,
      trust: false,
    });
  }
}

function updateEmptyState(view: EditorView) {
  view.dom.dataset.empty = String(
    view.state.doc.childCount === 1 &&
      view.state.doc.firstChild?.type.name === "paragraph" &&
      view.state.doc.firstChild.content.size === 0,
  );
}

async function installCodeBlockNodeView(view: EditorView): Promise<void> {
  if (view.props.nodeViews?.code_block || !documentHasCodeBlock(view.state.doc)) {
    return;
  }
  codeBlockNodeViewPromise ??= import("./CodeBlockNodeView").then(
    ({ codeBlockNodeView }) => codeBlockNodeView,
  );
  const constructor = await codeBlockNodeViewPromise;
  if (view.isDestroyed || view.props.nodeViews?.code_block) {
    return;
  }
  view.setProps({
    nodeViews: {
      ...view.props.nodeViews,
      code_block: constructor,
    },
  });
}

function documentHasCodeBlock(document: ProseMirrorNode): boolean {
  let found = false;
  document.descendants((node) => {
    if (node.type.name === "code_block") {
      found = true;
      return false;
    }
    return !found;
  });
  return found;
}
