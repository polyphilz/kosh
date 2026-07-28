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
      : [
          "li",
          {
            class: "kosh-task-list-item",
            "data-checked": String(checked),
            "data-task-item": "true",
          },
          [
            "input",
            {
              "aria-label": checked ? "Mark task incomplete" : "Mark task complete",
              checked: checked ? "checked" : undefined,
              class: "kosh-task-list-item__checkbox",
              contenteditable: "false",
              type: "checkbox",
            },
          ],
          ["div", { class: "kosh-task-list-item__content" }, 0],
        ];
  },
};

const markdownDefinition: NodeSpec = {
  atom: true,
  attrs: {
    identifier: { validate: "string" },
    label: { validate: "string" },
    title: { default: null, validate: "string|null" },
    url: { validate: "string" },
  },
  group: "block",
  parseDOM: [
    {
      tag: "div[data-kosh-markdown-definition]",
      getAttrs(dom) {
        const element = dom as HTMLElement;
        return {
          identifier: element.dataset.identifier ?? "",
          label: element.dataset.label ?? "",
          title: element.hasAttribute("data-title") ? (element.dataset.title ?? "") : null,
          url: element.dataset.url ?? "",
        };
      },
    },
  ],
  selectable: true,
  toDOM(node): DOMOutputSpec {
    const label = node.attrs.label as string;
    const title = node.attrs.title as string | null;
    const url = node.attrs.url as string;
    return [
      "div",
      {
        "aria-label": `Markdown link definition ${label}`,
        class: "kosh-markdown-definition",
        contenteditable: "false",
        "data-identifier": node.attrs.identifier,
        "data-kosh-markdown-definition": "true",
        "data-label": label,
        "data-title": title,
        "data-url": url,
      },
      `[${label}]: ${url}${title ? ` "${title}"` : ""}`,
    ];
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

const image: NodeSpec = {
  atom: true,
  attrs: {
    altText: { default: "", validate: "string" },
    attachmentId: { validate: "string" },
    caption: { default: "", validate: "string" },
    naturalHeight: { default: null, validate: "number|null" },
    naturalWidth: { default: null, validate: "number|null" },
    ocrError: { default: null, validate: "string|null" },
    ocrStatus: { default: "PENDING", validate: "string" },
    widthPercent: { default: 100, validate: "number" },
  },
  draggable: true,
  group: "block",
  parseDOM: [
    {
      tag: "figure[data-kosh-image]",
      getAttrs(dom) {
        const element = dom as HTMLElement;
        return {
          altText: element.dataset.altText ?? "",
          attachmentId: element.dataset.attachmentId ?? "",
          caption: element.dataset.caption ?? "",
          naturalHeight: Number(element.dataset.naturalHeight) || null,
          naturalWidth: Number(element.dataset.naturalWidth) || null,
          ocrError: element.dataset.ocrError ?? null,
          ocrStatus: element.dataset.ocrStatus ?? "PENDING",
          widthPercent: Number(element.dataset.widthPercent) || 100,
        };
      },
    },
  ],
  selectable: true,
  toDOM(node): DOMOutputSpec {
    return [
      "figure",
      {
        "data-alt-text": node.attrs.altText,
        "data-attachment-id": node.attrs.attachmentId,
        "data-caption": node.attrs.caption,
        "data-kosh-image": "true",
        "data-natural-height": node.attrs.naturalHeight,
        "data-natural-width": node.attrs.naturalWidth,
        "data-ocr-error": node.attrs.ocrError,
        "data-ocr-status": node.attrs.ocrStatus,
        "data-width-percent": node.attrs.widthPercent,
      },
    ];
  },
};

const pendingImage: NodeSpec = {
  atom: true,
  attrs: {
    label: { default: "Processing image", validate: "string" },
    requestId: { validate: "string" },
  },
  group: "block",
  selectable: false,
  toDOM(node): DOMOutputSpec {
    return [
      "div",
      {
        "aria-label": node.attrs.label,
        "data-kosh-image-pending": "true",
        role: "status",
      },
      `${node.attrs.label}…`,
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
nodes = nodes.addBefore("text", "kosh_image", image);
nodes = nodes.addBefore("text", "kosh_image_pending", pendingImage);
nodes = nodes.addBefore("text", "markdown_definition", markdownDefinition);

export const koshEditorSchema = new Schema({
  marks: basicSchema.spec.marks.update("link", safeLink).append({ strike }),
  nodes,
});
