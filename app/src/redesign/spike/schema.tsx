import {
  BlockNoteSchema,
  defaultBlockSpecs,
  defaultInlineContentSpecs,
  defaultStyleSpecs,
} from "@blocknote/core";
import { createReactBlockSpec, createReactInlineContentSpec } from "@blocknote/react";

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
    render: ({ block }) => (
      <div aria-label="Display math" className="kosh-spike-math kosh-spike-math--display">
        {block.props.latex || "∑ math"}
      </div>
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
    render: ({ inlineContent }) => (
      <span aria-label="Inline math" className="kosh-spike-math kosh-spike-math--inline">
        {inlineContent.props.latex || "math"}
      </span>
    ),
  },
);

export const koshSpikeSchema = BlockNoteSchema.create({
  blockSpecs: {
    paragraph: defaultBlockSpecs.paragraph,
    heading: heading(),
    bulletListItem: defaultBlockSpecs.bulletListItem,
    numberedListItem: defaultBlockSpecs.numberedListItem,
    codeBlock: defaultBlockSpecs.codeBlock,
    displayMath: displayMath(),
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

export type KoshSpikeEditor = typeof koshSpikeSchema.BlockNoteEditor;
export type KoshSpikePartialBlock = typeof koshSpikeSchema.PartialBlock;

export const initialSpikeBlocks: KoshSpikePartialBlock[] = [
  {
    type: "heading",
    props: { level: 1 },
    content: "BlockNote feasibility",
  },
  {
    type: "heading",
    props: { level: 2 },
    content: "Restricted heading two",
  },
  {
    type: "heading",
    props: { level: 3 },
    content: "Restricted heading three",
  },
  {
    type: "paragraph",
    content: [
      { type: "text", text: "Bold", styles: { bold: true } },
      { type: "text", text: ", italic", styles: { italic: true } },
      { type: "text", text: ", strike", styles: { strike: true } },
      { type: "text", text: ", and code", styles: { code: true } },
      { type: "text", text: " with ", styles: {} },
      { type: "inlineMath", props: { latex: "a_i" } },
      { type: "text", text: ".", styles: {} },
    ],
  },
  {
    type: "bulletListItem",
    content: "Parent bullet",
    children: [{ type: "bulletListItem", content: "Nested bullet" }],
  },
  {
    type: "numberedListItem",
    content: "Parent ordered item",
    children: [{ type: "numberedListItem", content: "Nested ordered item" }],
  },
  {
    type: "codeBlock",
    props: { language: "python" },
    content: "array = np.array([1, 2, 3])",
  },
  {
    type: "displayMath",
    props: { latex: "\\sum_i a_i" },
  },
  {
    type: "paragraph",
    content: "Type here to exercise input, composition, selection, and undo.",
  },
];

export const supportedSpikeBlockTypes = Object.freeze(Object.keys(koshSpikeSchema.blockSchema));
export const supportedSpikeInlineTypes = Object.freeze(
  Object.keys(koshSpikeSchema.inlineContentSchema),
);
export const supportedSpikeStyleTypes = Object.freeze(Object.keys(koshSpikeSchema.styleSchema));
