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

const attachment: NodeSpec = {
  atom: true,
  attrs: {
    attachmentId: { validate: "string" },
    displayFilename: { default: "PDF attachment", validate: "string" },
    extractedPageCount: { default: 0, validate: "number" },
    extractionError: { default: null, validate: "string|null" },
    extractionStatus: { default: "PENDING", validate: "string" },
    nextAttemptAtMs: { default: null, validate: "number|null" },
    pageCount: { default: 0, validate: "number" },
    unavailablePageCount: { default: 0, validate: "number" },
  },
  draggable: true,
  group: "block",
  parseDOM: [
    {
      tag: "div[data-kosh-attachment]",
      getAttrs(dom) {
        const element = dom as HTMLElement;
        return {
          attachmentId: element.dataset.attachmentId ?? "",
          displayFilename: element.dataset.displayFilename ?? "PDF attachment",
          extractedPageCount: Number(element.dataset.extractedPageCount) || 0,
          extractionError: element.dataset.extractionError ?? null,
          extractionStatus: element.dataset.extractionStatus ?? "PENDING",
          nextAttemptAtMs: Number(element.dataset.nextAttemptAtMs) || null,
          pageCount: Number(element.dataset.pageCount) || 0,
          unavailablePageCount: Number(element.dataset.unavailablePageCount) || 0,
        };
      },
    },
  ],
  selectable: true,
  toDOM(node): DOMOutputSpec {
    return [
      "div",
      {
        "data-attachment-id": node.attrs.attachmentId,
        "data-display-filename": node.attrs.displayFilename,
        "data-extracted-page-count": node.attrs.extractedPageCount,
        "data-extraction-error": node.attrs.extractionError,
        "data-extraction-status": node.attrs.extractionStatus,
        "data-next-attempt-at-ms": node.attrs.nextAttemptAtMs,
        "data-kosh-attachment": "true",
        "data-page-count": node.attrs.pageCount,
        "data-unavailable-page-count": node.attrs.unavailablePageCount,
      },
    ];
  },
};

const fileAttachment: NodeSpec = {
  atom: true,
  attrs: {
    attachmentId: { validate: "string" },
    byteLength: { default: 0, validate: "number" },
    caption: { default: "", validate: "string" },
    displayFilename: { default: "Attachment", validate: "string" },
    extractedLineCount: { default: 0, validate: "number" },
    extractionError: { default: null, validate: "string|null" },
    extractionStatus: { default: "NOT_APPLICABLE", validate: "string" },
    kind: { default: "BINARY", validate: "string" },
    mediaType: { default: "application/octet-stream", validate: "string" },
  },
  draggable: true,
  group: "block",
  parseDOM: [
    {
      tag: "section[data-kosh-file-attachment]",
      getAttrs(dom) {
        const element = dom as HTMLElement;
        return {
          attachmentId: element.dataset.attachmentId ?? "",
          byteLength: Number(element.dataset.byteLength) || 0,
          caption: element.dataset.caption ?? "",
          displayFilename: element.dataset.displayFilename ?? "Attachment",
          extractedLineCount: Number(element.dataset.extractedLineCount) || 0,
          extractionError: element.dataset.extractionError ?? null,
          extractionStatus: element.dataset.extractionStatus ?? "NOT_APPLICABLE",
          kind: element.dataset.kind ?? "BINARY",
          mediaType: element.dataset.mediaType ?? "application/octet-stream",
        };
      },
    },
  ],
  selectable: true,
  toDOM(node): DOMOutputSpec {
    return [
      "section",
      {
        "data-attachment-id": node.attrs.attachmentId,
        "data-byte-length": node.attrs.byteLength,
        "data-caption": node.attrs.caption,
        "data-display-filename": node.attrs.displayFilename,
        "data-extracted-line-count": node.attrs.extractedLineCount,
        "data-extraction-error": node.attrs.extractionError,
        "data-extraction-status": node.attrs.extractionStatus,
        "data-kind": node.attrs.kind,
        "data-kosh-file-attachment": "true",
        "data-media-type": node.attrs.mediaType,
      },
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
nodes = nodes.addBefore("text", "kosh_attachment", attachment);
nodes = nodes.addBefore("text", "kosh_file_attachment", fileAttachment);
nodes = nodes.addBefore("text", "markdown_definition", markdownDefinition);

export const koshEditorSchema = new Schema({
  marks: basicSchema.spec.marks.update("link", safeLink).append({ strike }),
  nodes,
});
