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
      <span className="kosh-math-editor kosh-math-editor--display" contentEditable={false}>
        <MathPreview display latex={block.props.latex} />
        <input
          aria-label="Display math source"
          className="kosh-math-editor__source"
          onChange={(event) =>
            editor.updateBlock(block, {
              props: { latex: event.currentTarget.value },
            })
          }
          onKeyDown={(event) => event.stopPropagation()}
          spellCheck={false}
          value={block.props.latex}
        />
      </span>
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
      <span className="kosh-math-editor kosh-math-editor--inline" contentEditable={false}>
        <MathPreview latex={inlineContent.props.latex} />
        <input
          aria-label="Inline math source"
          className="kosh-math-editor__source"
          onChange={(event) =>
            updateInlineContent({
              type: "inlineMath",
              props: { latex: event.currentTarget.value },
            })
          }
          onKeyDown={(event) => event.stopPropagation()}
          spellCheck={false}
          value={inlineContent.props.latex}
        />
      </span>
    ),
  },
);

const legacyMarkdown = createReactBlockSpec(
  {
    type: "legacyMarkdown",
    propSchema: {
      markdown: { default: "" },
    },
    content: "none",
  },
  {
    render: ({ block, editor }) => (
      <label className="kosh-legacy-markdown" contentEditable={false}>
        <span className="kosh-legacy-markdown__label">Legacy Markdown</span>
        <textarea
          aria-label="Legacy Markdown source"
          onChange={(event) =>
            editor.updateBlock(block, {
              props: { markdown: event.currentTarget.value },
            })
          }
          onKeyDown={(event) => event.stopPropagation()}
          spellCheck={false}
          value={block.props.markdown}
        />
      </label>
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
    legacyMarkdown: legacyMarkdown(),
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

function MathPreview({ display = false, latex }: { display?: boolean; latex: string }) {
  const html = renderToString(latex || "\\square", {
    displayMode: display,
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
