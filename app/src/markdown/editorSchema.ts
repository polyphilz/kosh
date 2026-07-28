import type { DOMOutputSpec, MarkSpec, NodeSpec } from "prosemirror-model";
import { Schema } from "prosemirror-model";
import { schema as basicSchema } from "prosemirror-schema-basic";
import { addListNodes, bulletList, listItem, orderedList } from "prosemirror-schema-list";
import { tableNodes } from "prosemirror-tables";
import { externalHttpUrl } from "./urlPolicy";

const codeBlock: NodeSpec = {
  ...basicSchema.spec.nodes.get("code_block"),
  attrs: {
    language: { default: null, validate: "string|null" },
  },
  parseDOM: [
    {
      tag: "pre",
      preserveWhitespace: "full",
      getAttrs(dom) {
        const element = dom as HTMLElement;
        const code = element.querySelector("code");
        const languageClass = (code?.className ?? "")
          .split(/\s+/)
          .find((name) => name.startsWith("language-"));
        return {
          language: element.dataset.language ?? languageClass?.slice(9) ?? null,
        };
      },
    },
  ],
  toDOM(node): DOMOutputSpec {
    const language = node.attrs.language as string | null;
    return [
      "pre",
      language ? { "data-language": language } : {},
      ["code", language ? { class: `language-${language}` } : {}, 0],
    ];
  },
};

const taskListItem: NodeSpec = {
  ...listItem,
  attrs: {
    checked: { default: null, validate: "boolean|null" },
  },
  content: "paragraph block*",
  parseDOM: [
    {
      tag: "li",
      getAttrs(dom) {
        const value = (dom as HTMLElement).dataset.checked;
        return { checked: value === undefined ? null : value === "true" };
      },
    },
  ],
  toDOM(node): DOMOutputSpec {
    const checked = node.attrs.checked as boolean | null;
    return checked === null
      ? ["li", 0]
      : ["li", { "data-checked": String(checked), "data-task-item": "true" }, 0];
  },
};

const mathInline: NodeSpec = {
  atom: true,
  attrs: { formula: { default: "", validate: "string" } },
  group: "inline",
  inline: true,
  parseDOM: [
    {
      tag: 'span[data-kosh-math="inline"]',
      getAttrs: (dom) => ({ formula: (dom as HTMLElement).dataset.formula ?? "" }),
    },
  ],
  toDOM(node): DOMOutputSpec {
    const formula = node.attrs.formula as string;
    return [
      "span",
      {
        "aria-label": `Math: ${formula}`,
        "data-kosh-math": "inline",
        "data-formula": formula,
        class: "kosh-math-node kosh-math-inline",
      },
      formula,
    ];
  },
};

const mathDisplay: NodeSpec = {
  atom: true,
  attrs: { formula: { default: "", validate: "string" } },
  group: "block",
  parseDOM: [
    {
      tag: 'div[data-kosh-math="display"]',
      getAttrs: (dom) => ({ formula: (dom as HTMLElement).dataset.formula ?? "" }),
    },
  ],
  toDOM(node): DOMOutputSpec {
    const formula = node.attrs.formula as string;
    return [
      "div",
      {
        "aria-label": `Display math: ${formula}`,
        "data-kosh-math": "display",
        "data-formula": formula,
        class: "kosh-math-node kosh-math-display",
      },
      formula,
    ];
  },
};

const strike: MarkSpec = {
  parseDOM: [{ tag: "del" }, { tag: "s" }, { tag: "strike" }],
  toDOM: (): DOMOutputSpec => ["del", 0],
};

const safeLink: MarkSpec = {
  attrs: {
    href: { validate: "string" },
    title: { default: null, validate: "string|null" },
  },
  inclusive: false,
  parseDOM: [
    {
      tag: "a[href]",
      getAttrs(dom) {
        const element = dom as HTMLAnchorElement;
        const href = externalHttpUrl(element.getAttribute("href") ?? undefined);
        return href ? { href, title: element.getAttribute("title") } : false;
      },
    },
  ],
  toDOM(mark): DOMOutputSpec {
    return ["a", { href: mark.attrs.href, title: mark.attrs.title, rel: "noopener noreferrer" }, 0];
  },
};

let nodes = basicSchema.spec.nodes.remove("image");
nodes = nodes.update("code_block", codeBlock);
nodes = addListNodes(nodes, "paragraph block*", "block");
nodes = nodes.update("ordered_list", {
  ...orderedList,
  content: "list_item+",
  group: "block",
});
nodes = nodes.update("bullet_list", {
  ...bulletList,
  content: "list_item+",
  group: "block",
});
nodes = nodes.update("list_item", taskListItem);
nodes = nodes.append(
  tableNodes({
    cellAttributes: {
      align: {
        default: null,
        getFromDOM(dom) {
          const value = dom.getAttribute("align") ?? dom.style.textAlign;
          return ["left", "center", "right"].includes(value) ? value : null;
        },
        setDOMAttr(value, attrs) {
          if (value) {
            attrs.style = `text-align: ${String(value)}`;
          }
        },
      },
    },
    cellContent: "paragraph",
    tableGroup: "block",
  }),
);
nodes = nodes.addBefore("text", "math_inline", mathInline);
nodes = nodes.addBefore("text", "math_display", mathDisplay);

export const koshEditorSchema = new Schema({
  marks: basicSchema.spec.marks.update("link", safeLink).append({ strike }),
  nodes,
});
