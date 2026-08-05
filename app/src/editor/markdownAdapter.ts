import type {
  BlockContent,
  Content,
  DefinitionContent,
  List,
  ListItem,
  PhrasingContent,
  Root,
  RootContent,
} from "mdast";
import { normalizeCodeLanguageLabel } from "../markdown/languages";
import { parseKoshMarkdownAst, serializeKoshMarkdownAst } from "../markdown/markdownAst";
import {
  parseKoshMediaToken,
  serializeKoshAttachmentToken,
  serializeKoshImageToken,
} from "../markdown/mediaTokens";
import { externalHttpUrl } from "../markdown/urlPolicy";
import type { KoshBlockNoteBlock, KoshBlockNotePartialBlock } from "./schema";

type AdapterBlock = {
  children?: AdapterBlock[];
  content?: unknown;
  props?: Record<string, unknown>;
  type?: string;
};

type TextStyles = Partial<Record<"bold" | "code" | "italic" | "strike", true>>;
type AdapterInline =
  | { styles: TextStyles; text: string; type: "text" }
  | {
      content: Array<{ styles: TextStyles; text: string; type: "text" }>;
      href: string;
      type: "link";
    }
  | { props: { latex: string }; type: "inlineMath" };

export function markdownToKoshBlocks(source: string): KoshBlockNotePartialBlock[] {
  const tree = parseKoshMarkdownAst(source);
  const blocks = tree.children.flatMap((node) => blockFromMarkdown(node, source));
  return blocks.length > 0 ? blocks : [{ type: "paragraph" }];
}

export function koshBlocksToMarkdown(
  blocks: readonly (KoshBlockNoteBlock | KoshBlockNotePartialBlock)[],
): string {
  const tree: Root = {
    type: "root",
    children: blocksToMarkdown(blocks as readonly AdapterBlock[]),
  };
  return serializeKoshMarkdownAst(tree, {
    distinctEmphasisMarker: needsDistinctEmphasisMarker(blocks as readonly AdapterBlock[]),
  });
}

function needsDistinctEmphasisMarker(blocks: readonly AdapterBlock[]): boolean {
  return blocks.some((block) => {
    if (needsDistinctEmphasisInContent(block.content)) return true;
    return needsDistinctEmphasisMarker(block.children ?? []);
  });
}

function needsDistinctEmphasisInContent(content: unknown): boolean {
  if (!Array.isArray(content)) return false;
  if (
    content.some((item) =>
      item && typeof item === "object"
        ? needsDistinctEmphasisInContent((item as Record<string, unknown>).content)
        : false,
    )
  ) {
    return true;
  }
  for (let index = 1; index < content.length; index += 1) {
    const left = textAttentionStyles(content[index - 1]);
    const right = textAttentionStyles(content[index]);
    if (left && right && left !== right) return true;
  }
  return false;
}

function textAttentionStyles(value: unknown): string | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (item.type !== "text" || !item.styles || typeof item.styles !== "object") return null;
  const styles = item.styles as Record<string, unknown>;
  const attention = `${styles.bold === true ? "b" : ""}${styles.italic === true ? "i" : ""}`;
  return attention || null;
}

function blockFromMarkdown(node: RootContent, source: string): KoshBlockNotePartialBlock[] {
  if (containsTitledLink(node)) return unsupportedMarkdown(node);
  if (containsUnsupportedInlineContext(node)) return unsupportedMarkdown(node);
  switch (node.type) {
    case "paragraph":
      if (node.children.length === 1 && node.children[0]?.type === "text") {
        const media = parseKoshMediaToken(node.children[0].value);
        if (media) return [mediaBlock(media)];
      }
      return [{ type: "paragraph", content: inlineFromMarkdown(node.children, source) }];
    case "heading":
      return node.depth <= 3
        ? [
            {
              type: "heading",
              props: { level: headingDepth(node.depth) },
              content: inlineFromMarkdown(node.children, source),
            },
          ]
        : unsupportedMarkdown(node);
    case "code":
      if (node.meta !== null && node.meta !== undefined) return unsupportedMarkdown(node);
      return [
        {
          type: "codeBlock",
          props: { language: canonicalCodeLanguage(node.lang) },
          content: node.value,
        },
      ];
    case "math":
      return [{ type: "displayMath", props: { latex: node.value } }];
    case "list":
      return canConvertList(node)
        ? node.children.map((item) => listItemFromMarkdown(item, node.ordered, source))
        : unsupportedMarkdown(node);
    case "html":
      return unsupportedMarkdown(node);
    default:
      return unsupportedMarkdown(node);
  }
}

function containsTitledLink(node: RootContent): boolean {
  const candidate = node as RootContent & {
    children?: RootContent[];
    identifier?: string;
    title?: string | null;
  };
  if (candidate.type === "link" && candidate.title !== null && candidate.title !== undefined) {
    return true;
  }
  return candidate.children?.some((child) => containsTitledLink(child)) ?? false;
}

function canConvertList(list: List): boolean {
  if (list.start !== null && list.start !== undefined && list.start !== 1) return false;
  return list.children.every((item) => {
    if (item.checked !== null && item.checked !== undefined) return false;
    const [first, ...rest] = item.children;
    return (
      first?.type === "paragraph" &&
      rest.every((child) => child.type === "list" && canConvertList(child))
    );
  });
}

function listItemFromMarkdown(
  item: ListItem,
  ordered: boolean | null | undefined,
  source: string,
): KoshBlockNotePartialBlock {
  const [paragraph, ...nestedLists] = item.children;
  if (paragraph?.type !== "paragraph") throw new Error("convertible list item has no paragraph");
  return {
    type: ordered ? "numberedListItem" : "bulletListItem",
    content: inlineFromMarkdown(paragraph.children, source),
    children: nestedLists.flatMap((list) => {
      if (list.type !== "list") return [];
      return list.children.map((child) => listItemFromMarkdown(child, list.ordered, source));
    }),
  };
}

function inlineFromMarkdown(
  nodes: readonly PhrasingContent[],
  source: string,
  styles: TextStyles = {},
): AdapterInline[] {
  return nodes.flatMap((node): AdapterInline[] => {
    switch (node.type) {
      case "text":
        return node.value ? [{ type: "text", text: node.value, styles }] : [];
      case "emphasis":
        return inlineFromMarkdown(node.children, source, { ...styles, italic: true });
      case "strong":
        return inlineFromMarkdown(node.children, source, { ...styles, bold: true });
      case "delete":
        return inlineFromMarkdown(node.children, source, { ...styles, strike: true });
      case "inlineCode":
        return node.value
          ? [{ type: "text", text: node.value, styles: { ...styles, code: true } }]
          : [];
      case "inlineMath":
        return [{ type: "inlineMath", props: { latex: node.value } }];
      case "break":
        return [{ type: "text", text: "\n", styles }];
      case "link": {
        const href =
          node.title === null || node.title === undefined ? externalHttpUrl(node.url) : null;
        const content = href ? styledTextFromMarkdown(node.children, source, styles) : null;
        return href && content
          ? [{ type: "link", href, content }]
          : literalInline(node, source, styles);
      }
      default:
        return literalInline(node, source, styles);
    }
  });
}

function styledTextFromMarkdown(
  nodes: readonly PhrasingContent[],
  source: string,
  styles: TextStyles,
): Array<{ styles: TextStyles; text: string; type: "text" }> | null {
  const content = inlineFromMarkdown(nodes, source, styles);
  return content.every((item) => item.type === "text") ? content : null;
}

function literalInline(node: PhrasingContent, source: string, styles: TextStyles): AdapterInline[] {
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  const text =
    start === undefined || end === undefined
      ? textFromMarkdownNode(node)
      : source.slice(start, end);
  return text ? [{ type: "text", text, styles }] : [];
}

function unsupportedMarkdown(node: RootContent): never {
  throw new Error(`Unsupported Markdown block: ${node.type}`);
}

function blocksToMarkdown(
  blocks: readonly AdapterBlock[],
): Array<BlockContent | DefinitionContent> {
  const children: Array<BlockContent | DefinitionContent> = [];
  for (let index = 0; index < blocks.length;) {
    const block = blocks[index]!;
    if (isListBlock(block)) {
      const ordered = block.type === "numberedListItem";
      const items: ListItem[] = [];
      while (index < blocks.length && blocks[index]?.type === block.type) {
        items.push(listItemToMarkdown(blocks[index]!));
        index += 1;
      }
      children.push({
        type: "list",
        ordered,
        start: ordered ? 1 : null,
        spread: false,
        children: items,
      });
      continue;
    }
    children.push(...blockToMarkdown(block));
    index += 1;
  }
  return children;
}

function blockToMarkdown(block: AdapterBlock): Array<BlockContent | DefinitionContent> {
  switch (block.type ?? "paragraph") {
    case "paragraph":
      return withFlattenedChildren(paragraphToMarkdown(block.content), block);
    case "heading":
      return withFlattenedChildren(
        [
          {
            type: "heading",
            depth: headingDepth(block.props?.level),
            children: inlineToMarkdown(block.content),
          },
        ],
        block,
      );
    case "codeBlock":
      return withFlattenedChildren(
        [
          {
            type: "code",
            lang: stringProp(block.props?.language) || null,
            meta: null,
            value: plainContent(block.content),
          },
        ],
        block,
      );
    case "displayMath":
      return withFlattenedChildren(
        [{ type: "math", value: stringProp(block.props?.latex) } as BlockContent],
        block,
      );
    case "koshImage":
      return withFlattenedChildren(
        [
          mediaParagraph(
            serializeKoshImageToken({
              attachmentId: stringProp(block.props?.attachmentId),
              altText: stringProp(block.props?.altText) || undefined,
              caption: stringProp(block.props?.caption) || undefined,
              widthPercent: numberProp(block.props?.widthPercent, 100),
            }),
          ),
        ],
        block,
      );
    case "koshFileAttachment":
      return withFlattenedChildren(
        [
          mediaParagraph(
            serializeKoshAttachmentToken(
              stringProp(block.props?.attachmentId),
              stringProp(block.props?.caption) || undefined,
            ),
          ),
        ],
        block,
      );
    case "koshPendingMedia":
      return blocksToMarkdown(block.children ?? []);
    default:
      if (isListBlock(block)) return [listForChildren([block])];
      throw new Error(`Cannot serialize Kosh block ${String(block.type)}`);
  }
}

function withFlattenedChildren(
  nodes: Array<BlockContent | DefinitionContent>,
  block: AdapterBlock,
): Array<BlockContent | DefinitionContent> {
  return [...nodes, ...blocksToMarkdown(block.children ?? [])];
}

function paragraphToMarkdown(content: unknown): BlockContent[] {
  const children = inlineToMarkdown(content);
  return children.length > 0 ? [{ type: "paragraph", children }] : [];
}

function listItemToMarkdown(block: AdapterBlock): ListItem {
  return {
    type: "listItem",
    checked: null,
    spread: false,
    children: [
      { type: "paragraph", children: inlineToMarkdown(block.content) },
      ...blocksToMarkdown(block.children ?? []).map(definitionToNestedBlock),
    ],
  };
}

function definitionToNestedBlock(node: BlockContent | DefinitionContent): BlockContent {
  if (isBlockContent(node)) return node;
  return {
    type: "paragraph",
    children: [
      {
        type: "text",
        value: serializeKoshMarkdownAst({ type: "root", children: [node] }),
      },
    ],
  };
}

function isBlockContent(node: BlockContent | DefinitionContent): node is BlockContent {
  return [
    "blockquote",
    "code",
    "heading",
    "html",
    "list",
    "math",
    "paragraph",
    "table",
    "thematicBreak",
  ].includes(node.type);
}

function listForChildren(blocks: readonly AdapterBlock[]): List {
  const ordered = blocks[0]?.type === "numberedListItem";
  return {
    type: "list",
    ordered,
    start: ordered ? 1 : null,
    spread: false,
    children: blocks.map(listItemToMarkdown),
  };
}

function inlineToMarkdown(content: unknown): PhrasingContent[] {
  if (!Array.isArray(content))
    return textWithBreaks(typeof content === "string" ? content : "", {});
  return content.flatMap((item): PhrasingContent[] => {
    if (!item || typeof item !== "object") return [];
    const value = item as Record<string, unknown>;
    if (value.type === "inlineMath") {
      const props = value.props as Record<string, unknown> | undefined;
      return [{ type: "inlineMath", value: stringProp(props?.latex) }];
    }
    if (value.type === "link") {
      const href = externalHttpUrl(stringProp(value.href));
      const linkContent = inlineToMarkdown(value.content);
      return href ? [{ type: "link", url: href, title: null, children: linkContent }] : linkContent;
    }
    if (value.type !== "text") return [];
    return styledTextToMarkdown(stringProp(value.text), value.styles);
  });
}

function styledTextToMarkdown(text: string, stylesValue: unknown): PhrasingContent[] {
  const styles =
    stylesValue && typeof stylesValue === "object" ? (stylesValue as Record<string, unknown>) : {};
  const pieces = text.split("\n");
  const nodes: PhrasingContent[] = [];
  pieces.forEach((piece, index) => {
    nodes.push(...styledLineToMarkdown(piece, styles));
    if (index < pieces.length - 1) nodes.push({ type: "break" });
  });
  return nodes;
}

function styledLineToMarkdown(text: string, styles: Record<string, unknown>): PhrasingContent[] {
  if (!text) return [];
  if (!Object.values(styles).some(Boolean)) return [{ type: "text", value: text }];
  if (styles.code) return [wrapStyles({ type: "inlineCode", value: text }, styles)];
  const leading = text.match(/^\s+/u)?.[0] ?? "";
  const withoutLeading = text.slice(leading.length);
  const trailing = withoutLeading.match(/\s+$/u)?.[0] ?? "";
  const core = withoutLeading.slice(0, withoutLeading.length - trailing.length);
  if (!core) return [wrapStyles({ type: "text", value: text }, styles)];
  return [
    ...(leading ? [{ type: "text" as const, value: leading }] : []),
    wrapStyles({ type: "text", value: core }, styles),
    ...(trailing ? [{ type: "text" as const, value: trailing }] : []),
  ];
}

function textWithBreaks(text: string, styles: Record<string, unknown>): PhrasingContent[] {
  const pieces = text.split("\n");
  const nodes: PhrasingContent[] = [];
  pieces.forEach((piece, index) => {
    if (piece) {
      nodes.push(
        styles.code ? { type: "inlineCode", value: piece } : { type: "text", value: piece },
      );
    }
    if (index < pieces.length - 1) nodes.push({ type: "break" });
  });
  return nodes;
}

function wrapStyles(node: PhrasingContent, styles: Record<string, unknown>): PhrasingContent {
  if (node.type === "break") return node;
  let wrapped = node;
  if (styles.strike) wrapped = { type: "delete", children: [wrapped] };
  if (styles.italic) wrapped = { type: "emphasis", children: [wrapped] };
  if (styles.bold) wrapped = { type: "strong", children: [wrapped] };
  return wrapped;
}

function plainContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((item) =>
      item && typeof item === "object" ? stringProp((item as Record<string, unknown>).text) : "",
    )
    .join("");
}

function isListBlock(block: AdapterBlock): boolean {
  return block.type === "bulletListItem" || block.type === "numberedListItem";
}

function headingDepth(value: unknown): 1 | 2 | 3 {
  return value === 2 || value === 3 ? value : 1;
}

function canonicalCodeLanguage(label: string | null | undefined): string {
  const sourceLabel = label?.trim().split(/\s+/, 1)[0]?.toLowerCase();
  return sourceLabel ? (normalizeCodeLanguageLabel(sourceLabel) ?? sourceLabel) : "";
}

function mediaBlock(
  media: NonNullable<ReturnType<typeof parseKoshMediaToken>>,
): KoshBlockNotePartialBlock {
  switch (media.kind) {
    case "image":
      return {
        type: "koshImage",
        props: {
          attachmentId: media.attachmentId,
          altText: media.altText ?? "",
          caption: media.caption ?? "",
          widthPercent: media.widthPercent,
        },
      };
    case "attachment":
      return {
        type: "koshFileAttachment",
        props: { attachmentId: media.attachmentId, caption: media.caption ?? "" },
      };
  }
}

function mediaParagraph(value: string): BlockContent {
  return { type: "paragraph", children: [{ type: "text", value }] };
}

function numberProp(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function stringProp(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function textFromMarkdownNode(node: Content): string {
  if ("value" in node && typeof node.value === "string") return node.value;
  return "children" in node ? (node.children as Content[]).map(textFromMarkdownNode).join("") : "";
}

function containsUnsupportedInlineContext(node: Content, decorated = false): boolean {
  if (["image", "imageReference", "linkReference"].includes(node.type)) return true;
  if (node.type === "inlineMath") return decorated;
  if (!("children" in node)) return false;
  const decoratesChildren =
    decorated || ["delete", "emphasis", "link", "linkReference", "strong"].includes(node.type);
  return (node.children as Content[]).some((child) =>
    containsUnsupportedInlineContext(child, decoratesChildren),
  );
}
