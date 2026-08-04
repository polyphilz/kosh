import {
  BlockNoteSchema,
  createCodeBlockSpec,
  defaultBlockSpecs,
  defaultInlineContentSpecs,
  defaultStyleSpecs,
} from "@blocknote/core";
import { createReactBlockSpec, createReactInlineContentSpec } from "@blocknote/react";
import { renderToString } from "katex";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { codeLanguageDefinitions } from "../markdown/languages";
import { useKoshEditorDisabled } from "./interactionState";
import { koshMediaBlockSpecs } from "./mediaBlocks";

const heading = createReactBlockSpec(
  {
    type: "heading",
    propSchema: {
      level: { default: 1, values: [1, 2, 3] as const },
    },
    content: "inline",
  },
  {
    render: ({ block, contentRef }) => {
      const Heading = `h${block.props.level}` as "h1" | "h2" | "h3";
      return <Heading data-kosh-block-type={`heading-${block.props.level}`} ref={contentRef} />;
    },
  },
);

const displayMath = createReactBlockSpec(
  {
    type: "displayMath",
    propSchema: {
      latex: { default: "" },
    },
    content: "none",
  },
  {
    render: ({ block, editor }) => (
      <MathSource
        display
        label="Display math source"
        latex={block.props.latex}
        onChange={(latex) => editor.updateBlock(block, { props: { latex } })}
      />
    ),
  },
);

const inlineMath = createReactInlineContentSpec(
  {
    type: "inlineMath",
    propSchema: {
      latex: { default: "" },
    },
    content: "none",
  },
  {
    render: ({ editor, inlineContent, updateInlineContent }) => (
      <MathSource
        label="Inline math source"
        latex={inlineContent.props.latex}
        onChange={(latex) =>
          updateInlineContent({
            type: "inlineMath",
            props: { latex },
          })
        }
        onRestoreCaret={(root) => {
          const position = editor._tiptapEditor.view.posAtDOM(root, 0);
          editor._tiptapEditor.commands.setTextSelection(position + 1);
          editor.focus();
        }}
      />
    ),
  },
);

const supportedCodeLanguages = Object.fromEntries(
  codeLanguageDefinitions.map((definition) => [
    definition.canonical,
    {
      name: definition.canonical,
      aliases: [...definition.aliases],
    },
  ]),
);

export const koshBlockNoteSchema = BlockNoteSchema.create({
  blockSpecs: {
    paragraph: defaultBlockSpecs.paragraph,
    heading: heading(),
    bulletListItem: defaultBlockSpecs.bulletListItem,
    numberedListItem: defaultBlockSpecs.numberedListItem,
    codeBlock: createCodeBlockSpec({
      defaultLanguage: "",
      indentLineWithTab: true,
      supportedLanguages: supportedCodeLanguages,
    }),
    displayMath: displayMath(),
    ...koshMediaBlockSpecs,
  },
  inlineContentSpecs: {
    text: defaultInlineContentSpecs.text,
    link: defaultInlineContentSpecs.link,
    inlineMath,
  },
  styleSpecs: {
    bold: defaultStyleSpecs.bold,
    italic: defaultStyleSpecs.italic,
    strike: defaultStyleSpecs.strike,
    code: defaultStyleSpecs.code,
  },
});

export type KoshBlockNoteBlock = typeof koshBlockNoteSchema.Block;
export type KoshBlockNoteEditor = typeof koshBlockNoteSchema.BlockNoteEditor;
export type KoshBlockNotePartialBlock = typeof koshBlockNoteSchema.PartialBlock;

export const supportedKoshBlockTypes = Object.freeze(Object.keys(koshBlockNoteSchema.blockSchema));
export const supportedKoshInlineTypes = Object.freeze(
  Object.keys(koshBlockNoteSchema.inlineContentSchema),
);
export const supportedKoshStyleTypes = Object.freeze(Object.keys(koshBlockNoteSchema.styleSchema));

function MathSource({
  display = false,
  label,
  latex,
  onChange,
  onRestoreCaret,
}: {
  display?: boolean;
  label: string;
  latex: string;
  onChange: (latex: string) => void;
  onRestoreCaret?: (root: HTMLElement) => void;
}) {
  const disabled = useKoshEditorDisabled();
  if (!display) {
    return (
      <InlineMathSource
        disabled={disabled}
        label={label}
        latex={latex}
        onChange={onChange}
        onRestoreCaret={onRestoreCaret!}
      />
    );
  }

  return (
    <span className="kosh-math-editor kosh-math-editor--display" contentEditable={false}>
      <MathPreview display latex={latex} />
      <textarea
        aria-label={label}
        className="kosh-math-editor__source"
        disabled={disabled}
        onChange={(event) => {
          if (!disabled) onChange(event.currentTarget.value);
        }}
        onKeyDown={(event) => event.stopPropagation()}
        rows={1}
        spellCheck={false}
        value={latex}
      />
    </span>
  );
}

function InlineMathSource({
  disabled,
  label,
  latex,
  onChange,
  onRestoreCaret,
}: {
  disabled: boolean;
  label: string;
  latex: string;
  onChange: (latex: string) => void;
  onRestoreCaret: (root: HTMLElement) => void;
}) {
  const [open, setOpen] = useState(false);
  const [popoverOffset, setPopoverOffset] = useState(0);
  const popoverRef = useRef<HTMLSpanElement>(null);
  const rootRef = useRef<HTMLSpanElement>(null);
  const sourceRef = useRef<HTMLInputElement>(null);
  const rendering = useMemo(() => renderMath(latex, false, true), [latex]);

  useLayoutEffect(() => {
    if (!open) return;
    const positionPopover = () => {
      const root = rootRef.current;
      const popover = popoverRef.current;
      if (!root || !popover) return;
      const viewportMargin = 16;
      const rootLeft = root.getBoundingClientRect().left;
      const popoverWidth = popover.getBoundingClientRect().width;
      const viewportWidth = document.documentElement.clientWidth;
      const maximumLeft = Math.max(viewportMargin, viewportWidth - viewportMargin - popoverWidth);
      const clampedLeft = Math.min(Math.max(rootLeft, viewportMargin), maximumLeft);
      setPopoverOffset(clampedLeft - rootLeft);
    };
    positionPopover();
    window.addEventListener("resize", positionPopover);
    return () => window.removeEventListener("resize", positionPopover);
  }, [latex, open]);

  useEffect(() => {
    if (!open) return;
    sourceRef.current?.focus();
    const sourceLength = sourceRef.current?.value.length ?? 0;
    sourceRef.current?.setSelectionRange(sourceLength, sourceLength);

    const closeOnOutsideClick = (event: MouseEvent) => {
      const root = rootRef.current;
      if (root && !event.composedPath().includes(root)) setOpen(false);
    };
    const closeOnFocusOutside = (event: FocusEvent) => {
      const root = rootRef.current;
      if (root && !event.composedPath().includes(root)) setOpen(false);
    };
    document.addEventListener("click", closeOnOutsideClick);
    document.addEventListener("focusin", closeOnFocusOutside);
    return () => {
      document.removeEventListener("click", closeOnOutsideClick);
      document.removeEventListener("focusin", closeOnFocusOutside);
    };
  }, [open]);

  const close = () => {
    setOpen(false);
    window.requestAnimationFrame(() => {
      const root = rootRef.current;
      if (root) onRestoreCaret(root);
    });
  };
  const accessibleEquation = rendering.error ? "Invalid equation" : latex || "empty equation";

  return (
    <span
      className={`kosh-math-editor kosh-math-editor--inline${open ? " kosh-math-editor--open" : ""}`}
      contentEditable={false}
      ref={rootRef}
    >
      <span
        aria-disabled={disabled}
        aria-expanded={open}
        aria-label={`Edit inline math: ${accessibleEquation}`}
        className="kosh-math-editor__trigger"
        onClick={() => {
          if (!disabled) setOpen(true);
        }}
        onKeyDown={(event) => {
          if (disabled || (event.key !== "Enter" && event.key !== " ")) return;
          event.preventDefault();
          event.stopPropagation();
          setOpen(true);
        }}
        role="button"
        tabIndex={disabled ? -1 : 0}
      >
        {rendering.error ? (
          <span className="kosh-math-editor__invalid-preview">√x Invalid equation</span>
        ) : (
          <MathMarkup html={rendering.html} />
        )}
      </span>
      {open && (
        <span
          aria-label="Edit inline math"
          className="kosh-math-editor__popover"
          ref={popoverRef}
          role="dialog"
          style={{ transform: `translateX(${popoverOffset}px)` }}
        >
          <span className="kosh-math-editor__controls">
            <input
              aria-label={label}
              className="kosh-math-editor__source"
              disabled={disabled}
              onChange={(event) => {
                if (!disabled) onChange(event.currentTarget.value);
              }}
              onKeyDown={(event) => {
                event.stopPropagation();
                if (event.key === "Escape") {
                  event.preventDefault();
                  close();
                } else if (event.key === "Enter") {
                  event.preventDefault();
                  if (!rendering.error) close();
                }
              }}
              ref={sourceRef}
              spellCheck={false}
              value={latex}
            />
            <button
              className="kosh-math-editor__done"
              disabled={disabled || Boolean(rendering.error)}
              onClick={close}
              type="button"
            >
              Done <span aria-hidden>↵</span>
            </button>
          </span>
          {rendering.error && (
            <span className="kosh-math-editor__error" role="alert">
              <strong>Invalid equation:</strong> {rendering.error}
            </span>
          )}
        </span>
      )}
    </span>
  );
}

function MathPreview({ display = false, latex }: { display?: boolean; latex: string }) {
  return <MathMarkup html={renderMath(latex, display, false).html} />;
}

function MathMarkup({ html }: { html: string }) {
  return (
    <span
      aria-hidden
      className="kosh-math-editor__preview"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

function renderMath(
  latex: string,
  display: boolean,
  validate: boolean,
): { error: string | null; html: string } {
  const options = {
    displayMode: display,
    maxSize: 20,
    output: "html",
    strict: "ignore",
    throwOnError: validate,
    trust: false,
  } as const;
  try {
    return { error: null, html: renderToString(latex || "\\square", options) };
  } catch (error) {
    const message =
      error instanceof Error
        ? error.message.replace(/^KaTeX parse error:\s*/u, "").slice(0, 240)
        : "The equation could not be parsed.";
    return {
      error: message,
      html: renderToString("\\text{Invalid equation}", { ...options, throwOnError: false }),
    };
  }
}
