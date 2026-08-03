import {
  koshBlockNoteSchema,
  supportedKoshInlineTypes,
  supportedKoshStyleTypes,
  type KoshBlockNoteEditor,
  type KoshBlockNotePartialBlock,
} from "../../editor/schema";

export const koshSpikeSchema = koshBlockNoteSchema;
export type KoshSpikeEditor = KoshBlockNoteEditor;
export type KoshSpikePartialBlock = KoshBlockNotePartialBlock;

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

export const supportedSpikeBlockTypes = Object.freeze([
  "paragraph",
  "heading",
  "bulletListItem",
  "numberedListItem",
  "codeBlock",
  "displayMath",
]);
export const supportedSpikeInlineTypes = supportedKoshInlineTypes;
export const supportedSpikeStyleTypes = supportedKoshStyleTypes;
