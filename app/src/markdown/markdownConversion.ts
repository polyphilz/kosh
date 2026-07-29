import type {
  BlockContent,
  Break,
  Content,
  Definition,
  DefinitionContent,
  ListItem,
  PhrasingContent,
  Root,
  RootContent,
  Table,
  TableCell,
} from "mdast";
import type { Mark, Node as ProseMirrorNode, Schema } from "prosemirror-model";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkStringify from "remark-stringify";
import { unified } from "unified";
import { normalizeCodeLanguageLabel } from "./languages";
import {
  parseKoshMediaToken,
  serializeKoshAttachmentToken,
  serializeKoshImageToken,
  serializeKoshPdfToken,
} from "./mediaTokens";
import { externalHttpUrl } from "./urlPolicy";

const markdownParser = unified().use(remarkParse).use(remarkGfm).use(remarkMath).use(remarkBreaks);

const markdownSerializer = unified()
  .use(remarkGfm)
  .use(remarkMath)
  .use(remarkStringify, {
    bullet: "-",
    bulletOrdered: ".",
    emphasis: "*",
    fences: true,
    incrementListMarker: true,
    listItemIndent: "one",
    resourceLink: true,
    rule: "-",
    strong: "*",
    handlers: {
      break(_node, _parent, state) {
        return state.stack.includes("tableCell") ? " " : "\n";
      },
    },
  });

export function parseKoshMarkdownAst(source: string): Root {
  const parsed = markdownParser.parse(source);
  return markdownParser.runSync(parsed) as Root;
}

export function parseKoshMarkdown(source: string, schema: Schema): ProseMirrorNode {
  const tree = parseKoshMarkdownAst(source);
  const definitions = new Map<string, MarkdownDefinition>();
  visitMarkdown(tree, (node) => {
    if (node.type === "definition" && !definitions.has(node.identifier.toLowerCase())) {
      definitions.set(node.identifier.toLowerCase(), {
        node,
        title: node.title ?? null,
        url: node.url,
      });
    }
  });
  const context: MarkdownContext = {
    consumedDefinitions: new Set(),
    definitions,
  };
  visitMarkdown(tree, (node) => {
    if (node.type === "linkReference") {
      const definition = definitions.get(node.identifier.toLowerCase());
      if (definition && externalHttpUrl(definition.url)) {
        context.consumedDefinitions.add(definition.node);
      }
    }
  });
  const blocks = tree.children.flatMap((node) => blockFromMarkdown(node, source, schema, context));
  return schema.topNodeType.createAndFill(null, blocks.length ? blocks : null)!;
}

function visitMarkdown(node: Content | Root, visitor: (node: Content) => void): void {
  if (node.type !== "root") {
    visitor(node);
  }
  if ("children" in node) {
    for (const child of node.children as Content[]) {
      visitMarkdown(child, visitor);
    }
  }
}

export function serializeKoshMarkdown(document: ProseMirrorNode): string {
  const tree: Root = {
    children: document.content.content.flatMap(blockToMarkdown),
    type: "root",
  };
  return markdownSerializer.stringify(tree).replace(/\n$/u, "");
}

function blockFromMarkdown(
  node: RootContent | DefinitionContent,
  source: string,
  schema: Schema,
  context: MarkdownContext,
): ProseMirrorNode[] {
  switch (node.type) {
    case "paragraph":
      if (node.children.length === 1 && node.children[0]?.type === "text") {
        const media = parseKoshMediaToken(node.children[0].value);
        if (media?.kind === "image") {
          return [
            schema.nodes.kosh_image!.create({
              altText: media.altText ?? "",
              attachmentId: media.attachmentId,
              caption: media.caption ?? "",
              naturalHeight: null,
              naturalWidth: null,
              ocrError: null,
              ocrStatus: "PENDING",
              widthPercent: media.widthPercent,
            }),
          ];
        }
        if (media?.kind === "pdf") {
          return [
            schema.nodes.kosh_attachment!.create({
              attachmentId: media.attachmentId,
              displayFilename: "PDF attachment",
              extractedPageCount: 0,
              extractionError: null,
              extractionStatus: "PENDING",
              nextAttemptAtMs: null,
              pageCount: 0,
              unavailablePageCount: 0,
            }),
          ];
        }
        if (media?.kind === "attachment") {
          return [
            schema.nodes.kosh_file_attachment!.create({
              attachmentId: media.attachmentId,
              byteLength: 0,
              caption: media.caption ?? "",
              displayFilename: "Attachment",
              extractedLineCount: 0,
              extractionError: null,
              extractionStatus: "NOT_APPLICABLE",
              kind: "BINARY",
              mediaType: "application/octet-stream",
            }),
          ];
        }
      }
      return [
        schema.nodes.paragraph!.create(
          null,
          inlineFromMarkdown(node.children, source, schema, context),
        ),
      ];
    case "heading":
      return [
        schema.nodes.heading!.create(
          { level: node.depth },
          inlineFromMarkdown(node.children, source, schema, context),
        ),
      ];
    case "blockquote":
      return [
        schema.nodes.blockquote!.create(
          null,
          node.children.flatMap((child) => blockFromMarkdown(child, source, schema, context)),
        ),
      ];
    case "thematicBreak":
      return [schema.nodes.horizontal_rule!.create()];
    case "code":
      return [
        schema.nodes.code_block!.create(
          { language: canonicalCodeLanguage(node.lang) },
          node.value ? schema.text(node.value) : null,
        ),
      ];
    case "math":
      return [schema.nodes.math_display!.create({ formula: node.value })];
    case "list": {
      const type = node.ordered ? schema.nodes.ordered_list! : schema.nodes.bullet_list!;
      return [
        type.create(
          node.ordered ? { order: node.start ?? 1 } : null,
          node.children.map((item) => listItemFromMarkdown(item, source, schema, context)),
        ),
      ];
    }
    case "table":
      return [tableFromMarkdown(node, source, schema, context)];
    case "html":
      return [schema.nodes.paragraph!.create(null, schema.text(node.value))];
    case "definition":
      return context.consumedDefinitions.has(node) ? [] : [definitionFromMarkdown(node, schema)];
    default:
      return fallbackBlock(node, source, schema);
  }
}

function definitionFromMarkdown(definition: Definition, schema: Schema): ProseMirrorNode {
  return schema.nodes.markdown_definition!.create({
    identifier: definition.identifier,
    label: definition.label ?? definition.identifier,
    title: definition.title ?? null,
    url: definition.url,
  });
}

function canonicalCodeLanguage(label: string | null | undefined): string | null {
  const sourceLabel = label?.trim().split(/\s+/, 1)[0]?.toLowerCase();
  return sourceLabel ? (normalizeCodeLanguageLabel(sourceLabel) ?? sourceLabel) : null;
}

function listItemFromMarkdown(
  item: ListItem,
  source: string,
  schema: Schema,
  context: MarkdownContext,
): ProseMirrorNode {
  const blocks = item.children.flatMap((child) =>
    blockFromMarkdown(child, source, schema, context),
  );
  return schema.nodes.list_item!.create(
    { checked: item.checked ?? null },
    blocks.length ? blocks : schema.nodes.paragraph!.create(),
  );
}

function tableFromMarkdown(
  table: Table,
  source: string,
  schema: Schema,
  context: MarkdownContext,
): ProseMirrorNode {
  return schema.nodes.table!.create(
    null,
    table.children.map((row, rowIndex) =>
      schema.nodes.table_row!.create(
        null,
        row.children.map((cell, columnIndex) =>
          tableCellFromMarkdown(
            cell,
            rowIndex === 0,
            table.align?.[columnIndex] ?? null,
            source,
            schema,
            context,
          ),
        ),
      ),
    ),
  );
}

function tableCellFromMarkdown(
  cell: TableCell,
  header: boolean,
  align: "center" | "left" | "right" | null,
  source: string,
  schema: Schema,
  context: MarkdownContext,
): ProseMirrorNode {
  const type = header ? schema.nodes.table_header! : schema.nodes.table_cell!;
  return type.createAndFill(
    { align },
    schema.nodes.paragraph!.create(
      null,
      inlineFromMarkdown(cell.children, source, schema, context),
    ),
  )!;
}

function inlineFromMarkdown(
  children: readonly PhrasingContent[],
  source: string,
  schema: Schema,
  context: MarkdownContext,
  marks: readonly Mark[] = [],
): ProseMirrorNode[] {
  return children.flatMap((node) => {
    switch (node.type) {
      case "text":
        return node.value ? [schema.text(node.value, marks)] : [];
      case "emphasis":
        return inlineFromMarkdown(node.children, source, schema, context, [
          ...marks,
          schema.marks.em!.create(),
        ]);
      case "strong":
        return inlineFromMarkdown(node.children, source, schema, context, [
          ...marks,
          schema.marks.strong!.create(),
        ]);
      case "delete":
        return inlineFromMarkdown(node.children, source, schema, context, [
          ...marks,
          schema.marks.strike!.create(),
        ]);
      case "link": {
        const href = externalHttpUrl(node.url);
        return href
          ? inlineFromMarkdown(node.children, source, schema, context, [
              ...marks,
              schema.marks.link!.create({ href, title: node.title ?? null }),
            ])
          : literalInlineNode(node, source, schema, marks);
      }
      case "inlineCode":
        return node.value ? [schema.text(node.value, [...marks, schema.marks.code!.create()])] : [];
      case "inlineMath":
        return [schema.nodes.math_inline!.create({ formula: node.value }, null, marks)];
      case "break":
        return [schema.nodes.hard_break!.create(null, null, marks)];
      case "linkReference": {
        const definition = context.definitions.get(node.identifier.toLowerCase());
        const href = externalHttpUrl(definition?.url);
        if (!definition || !href) {
          return literalInlineNode(node, source, schema, marks);
        }
        return inlineFromMarkdown(node.children, source, schema, context, [
          ...marks,
          schema.marks.link!.create({ href, title: definition.title }),
        ]);
      }
      case "html":
        return node.value ? [schema.text(node.value, marks)] : [];
      case "image":
      case "imageReference":
        return literalInlineNode(node, source, schema, marks);
      default:
        return literalInlineNode(node, source, schema, marks);
    }
  });
}

interface MarkdownDefinition {
  node: Definition;
  title: string | null;
  url: string;
}

interface MarkdownContext {
  consumedDefinitions: Set<Definition>;
  definitions: ReadonlyMap<string, MarkdownDefinition>;
}

function literalInlineNode(
  node: PhrasingContent,
  source: string,
  schema: Schema,
  marks: readonly Mark[],
): ProseMirrorNode[] {
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  const value =
    start === undefined || end === undefined
      ? textFromMarkdownNode(node)
      : source.slice(start, end);
  return value ? [schema.text(value, marks)] : [];
}

function fallbackBlock(
  node: RootContent | DefinitionContent,
  source: string,
  schema: Schema,
): ProseMirrorNode[] {
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  const value =
    start === undefined || end === undefined
      ? textFromMarkdownNode(node)
      : source.slice(start, end);
  return [schema.nodes.paragraph!.create(null, value ? schema.text(value) : null)];
}

function textFromMarkdownNode(node: Content): string {
  if ("value" in node && typeof node.value === "string") {
    return node.value;
  }
  return "children" in node ? (node.children as Content[]).map(textFromMarkdownNode).join("") : "";
}

function blockToMarkdown(node: ProseMirrorNode): Array<BlockContent | DefinitionContent> {
  switch (node.type.name) {
    case "paragraph":
      return [{ type: "paragraph", children: inlineToMarkdown(node) }];
    case "heading":
      return [
        {
          type: "heading",
          depth: node.attrs.level,
          children: inlineToMarkdown(node),
        },
      ];
    case "blockquote":
      return [
        {
          type: "blockquote",
          children: node.content.content.flatMap(blockToMarkdown),
        },
      ];
    case "horizontal_rule":
      return [{ type: "thematicBreak" }];
    case "code_block":
      return [
        {
          type: "code",
          lang: node.attrs.language ?? null,
          meta: null,
          value: node.textContent,
        },
      ];
    case "math_display":
      return [{ type: "math", value: node.attrs.formula } as BlockContent];
    case "markdown_definition":
      return [
        {
          type: "definition",
          identifier: node.attrs.identifier,
          label: node.attrs.label,
          title: node.attrs.title ?? null,
          url: node.attrs.url,
        },
      ];
    case "ordered_list":
    case "bullet_list":
      return [
        {
          type: "list",
          ordered: node.type.name === "ordered_list",
          start: node.type.name === "ordered_list" ? (node.attrs.order ?? 1) : null,
          spread: false,
          children: node.content.content.map(listItemToMarkdown),
        },
      ];
    case "table":
      return [tableToMarkdown(node)];
    case "kosh_image":
      return [
        {
          type: "paragraph",
          children: [
            {
              type: "text",
              value: serializeKoshImageToken({
                altText: node.attrs.altText || undefined,
                attachmentId: node.attrs.attachmentId,
                caption: node.attrs.caption || undefined,
                widthPercent: node.attrs.widthPercent,
              }),
            },
          ],
        },
      ];
    case "kosh_image_pending":
      return [];
    case "kosh_attachment":
      return [
        {
          type: "paragraph",
          children: [
            {
              type: "text",
              value: serializeKoshPdfToken(node.attrs.attachmentId),
            },
          ],
        },
      ];
    case "kosh_file_attachment":
      return [
        {
          type: "paragraph",
          children: [
            {
              type: "text",
              value: serializeKoshAttachmentToken(
                node.attrs.attachmentId,
                node.attrs.caption || undefined,
              ),
            },
          ],
        },
      ];
    default:
      throw new Error(`Cannot serialize editor node ${node.type.name}`);
  }
}

function listItemToMarkdown(node: ProseMirrorNode): ListItem {
  return {
    type: "listItem",
    checked: node.attrs.checked ?? null,
    spread: false,
    children: node.content.content.flatMap(blockToMarkdown),
  };
}

function tableToMarkdown(node: ProseMirrorNode): Table {
  return {
    type: "table",
    align: node.firstChild
      ? node.firstChild.content.content.map((cell) => cell.attrs.align ?? null)
      : [],
    children: node.content.content.map((row) => ({
      type: "tableRow",
      children: row.content.content.map((cell) => ({
        type: "tableCell",
        children: cellInlineToMarkdown(cell),
      })),
    })),
  };
}

function cellInlineToMarkdown(cell: ProseMirrorNode): PhrasingContent[] {
  const result: PhrasingContent[] = [];
  cell.forEach((block, _offset, index) => {
    if (index > 0) {
      result.push({ type: "break" } as Break);
    }
    result.push(...inlineToMarkdown(block));
  });
  return result;
}

function inlineToMarkdown(node: ProseMirrorNode): PhrasingContent[] {
  const output: PhrasingContent[] = [];
  node.forEach((child) => {
    let value: PhrasingContent;
    if (child.isText) {
      const code = child.marks.some((mark) => mark.type.name === "code");
      value = code
        ? { type: "inlineCode", value: child.text ?? "" }
        : { type: "text", value: child.text ?? "" };
    } else if (child.type.name === "hard_break") {
      value = { type: "break" };
    } else if (child.type.name === "math_inline") {
      value = { type: "inlineMath", value: child.attrs.formula } as PhrasingContent;
    } else {
      throw new Error(`Cannot serialize inline editor node ${child.type.name}`);
    }

    const nonCodeMarks = child.marks.filter((mark) => mark.type.name !== "code");
    for (let index = nonCodeMarks.length - 1; index >= 0; index -= 1) {
      value = wrapMark(value, nonCodeMarks[index]!);
    }
    appendMerged(output, value);
  });
  return output;
}

function wrapMark(child: PhrasingContent, mark: Mark): PhrasingContent {
  switch (mark.type.name) {
    case "strong":
      return { type: "strong", children: [child] };
    case "em":
      return { type: "emphasis", children: [child] };
    case "strike":
      return { type: "delete", children: [child] };
    case "link":
      return {
        type: "link",
        url: mark.attrs.href,
        title: mark.attrs.title ?? null,
        children: [child],
      };
    default:
      return child;
  }
}

function appendMerged(output: PhrasingContent[], value: PhrasingContent): void {
  const previous = output[output.length - 1];
  if (!previous || previous.type !== value.type) {
    output.push(value);
    return;
  }
  if (previous.type === "text" && value.type === "text") {
    previous.value += value.value;
    return;
  }
  if (previous.type === "inlineCode" && value.type === "inlineCode") {
    previous.value += value.value;
    return;
  }
  if ("children" in previous && "children" in value && sameWrapper(previous, value)) {
    previous.children.push(...value.children);
    return;
  }
  output.push(value);
}

function sameWrapper(left: PhrasingContent, right: PhrasingContent): boolean {
  if (left.type !== right.type) {
    return false;
  }
  if (left.type === "link" && right.type === "link") {
    return left.url === right.url && left.title === right.title;
  }
  return ["strong", "emphasis", "delete"].includes(left.type);
}
