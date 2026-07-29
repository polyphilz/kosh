import type { Content, Root } from "mdast";
import { parseKoshMediaToken } from "./mediaTokens";
import { parseKoshMarkdownAst } from "./markdownConversion";

export function markdownToPlainText(source: string): string {
  return renderNode(parseKoshMarkdownAst(source))
    .replace(/[ \t]+\n/gu, "\n")
    .replace(/\n{3,}/gu, "\n\n")
    .trim();
}

function renderNode(node: Root | Content): string {
  switch (node.type) {
    case "root":
      return node.children.map(renderNode).join("\n\n");
    case "text": {
      const media = parseKoshMediaToken(node.value);
      if (!media) return node.value;
      return media.kind === "image" ? (media.altText ?? "Image attachment") : "Attachment";
    }
    case "code":
    case "inlineCode":
    case "math":
    case "inlineMath":
    case "html":
      return node.value;
    case "break":
      return "\n";
    case "thematicBreak":
      return "—";
    case "image":
      return node.alt || "Image";
    case "imageReference":
      return node.alt || "Image";
    case "definition":
    case "footnoteDefinition":
      return "";
    case "list":
      return node.children
        .map(
          (child, index) =>
            `${node.ordered ? `${index + (node.start ?? 1)}.` : "•"} ${renderNode(child)}`,
        )
        .join("\n");
    case "table":
      return node.children.map(renderNode).join("\n");
    case "tableRow":
      return node.children.map(renderNode).join("\t");
    default:
      return "children" in node ? node.children.map(renderNode).join("") : "";
  }
}
