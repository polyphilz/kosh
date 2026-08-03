import type { Root } from "mdast";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkStringify from "remark-stringify";
import { unified } from "unified";

const markdownParser = unified().use(remarkParse).use(remarkGfm).use(remarkMath).use(remarkBreaks);

function createMarkdownSerializer(emphasis: "*" | "_") {
  return unified()
    .use(remarkGfm)
    .use(remarkMath)
    .use(remarkStringify, {
      bullet: "-",
      bulletOrdered: ".",
      emphasis,
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
}

const markdownSerializer = createMarkdownSerializer("*");
const distinctEmphasisSerializer = createMarkdownSerializer("_");

export function parseKoshMarkdownAst(source: string): Root {
  const parsed = markdownParser.parse(source);
  return markdownParser.runSync(parsed) as Root;
}

export function serializeKoshMarkdownAst(
  tree: Root,
  options: { distinctEmphasisMarker?: boolean } = {},
): string {
  const serializer = options.distinctEmphasisMarker
    ? distinctEmphasisSerializer
    : markdownSerializer;
  return serializer.stringify(tree).replace(/\n$/u, "");
}
