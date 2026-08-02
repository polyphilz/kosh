import {
  BlockNoteSchema,
  createCodeBlockSpec,
  defaultBlockSpecs,
  defaultInlineContentSpecs,
  defaultStyleSpecs,
} from "@blocknote/core";
import { createReactBlockSpec, createReactInlineContentSpec } from "@blocknote/react";
import { renderToString } from "katex";
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
    render: ({ inlineContent, updateInlineContent }) => (
      <MathSource
        label="Inline math source"
        latex={inlineContent.props.latex}
        onChange={(latex) =>
          updateInlineContent({
            type: "inlineMath",
            props: { latex },
          })
        }
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
}: {
  display?: boolean;
  label: string;
  latex: string;
  onChange: (latex: string) => void;
}) {
  const disabled = useKoshEditorDisabled();
  return (
    <span
      className={`kosh-math-editor kosh-math-editor--${display ? "display" : "inline"}`}
      contentEditable={false}
    >
      <MathPreview display={display} latex={latex} />
      {display ? (
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
      ) : (
        <input
          aria-label={label}
          className="kosh-math-editor__source"
          disabled={disabled}
          onChange={(event) => {
            if (!disabled) onChange(event.currentTarget.value);
          }}
          onKeyDown={(event) => event.stopPropagation()}
          spellCheck={false}
          value={latex}
        />
      )}
    </span>
  );
}

function MathPreview({ display = false, latex }: { display?: boolean; latex: string }) {
  const html = renderToString(latex || "\\square", {
    displayMode: display,
    maxSize: 20,
    output: "html",
    strict: "ignore",
    throwOnError: false,
    trust: false,
  });
  return (
    <span
      aria-hidden
      className="kosh-math-editor__preview"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
