import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import type { Options as ReactMarkdownOptions } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import rehypeKatex from "rehype-katex";
import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from "rehype-sanitize";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { codeLanguageAliases, normalizeCodeLanguageLabel } from "./languages";
import { parseKoshMediaToken } from "./mediaTokens";
import { attachmentMediaUrl } from "../media/gateway";

export const remarkPlugins: NonNullable<ReactMarkdownOptions["remarkPlugins"]> = [
  remarkInertHtml,
  remarkKoshImages,
  remarkGfm,
  remarkMath,
  remarkBreaks,
];

const sanitizeSchema: SanitizeSchema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    code: [
      ...(defaultSchema.attributes?.code ?? []),
      ["className", "math-inline", "math-display", /^language-[\w+-]+$/u],
    ],
    img: ["alt", "src", "title"],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: ["http", "https"],
    src: ["kosh-media"],
  },
};

const highlightLanguages = {
  bash,
  c,
  cpp,
  css,
  go,
  html: xml,
  java,
  javascript,
  json,
  jsx: javascript,
  kotlin,
  markdown,
  python,
  rust,
  sql,
  swift,
  toml: ini,
  tsx: typescript,
  typescript,
  xml,
  yaml,
};

export const rehypePlugins: NonNullable<ReactMarkdownOptions["rehypePlugins"]> = [
  [rehypeSanitize, sanitizeSchema],
  normalizeCodeLanguageClasses,
  [
    rehypeHighlight,
    {
      aliases: codeLanguageAliases,
      detect: false,
      languages: highlightLanguages,
    },
  ],
  [
    rehypeKatex,
    {
      maxExpand: 1_000,
      maxSize: 20,
      strict: "warn",
      trust: false,
    },
  ],
];

interface MarkdownAstNode {
  children?: MarkdownAstNode[];
  alt?: string;
  title?: string;
  type: string;
  url?: string;
  value?: string;
}

function remarkKoshImages() {
  return (tree: unknown) => {
    visitKoshImageNodes(tree as MarkdownAstNode);
  };
}

function visitKoshImageNodes(node: MarkdownAstNode) {
  if (!node.children) {
    return;
  }
  for (let index = 0; index < node.children.length; index += 1) {
    const child = node.children[index]!;
    if (
      child.type === "paragraph" &&
      child.children?.length === 1 &&
      child.children[0]?.type === "text"
    ) {
      const token = parseKoshMediaToken(child.children[0].value ?? "");
      if (token?.kind === "image") {
        node.children[index] = {
          alt: token.altText ?? "",
          title: `kosh-image:${token.widthPercent}:${encodeURIComponent(token.caption ?? "")}`,
          type: "image",
          url: attachmentMediaUrl(token.attachmentId),
        };
        continue;
      }
    }
    visitKoshImageNodes(child);
  }
}

function remarkInertHtml() {
  return (tree: unknown) => {
    visitMarkdownNodes(tree as MarkdownAstNode);
  };
}

function visitMarkdownNodes(node: MarkdownAstNode) {
  if (!node.children) {
    return;
  }
  for (let index = 0; index < node.children.length; index += 1) {
    const child = node.children[index]!;
    if (child.type === "html") {
      node.children[index] = {
        type: "text",
        value: child.value ?? "",
      };
    } else {
      visitMarkdownNodes(child);
    }
  }
}

interface HastElement {
  children?: HastElement[];
  properties?: Record<string, unknown>;
}

function normalizeCodeLanguageClasses() {
  return (tree: unknown) => {
    visitHastNodes(tree as HastElement);
  };
}

function visitHastNodes(node: HastElement) {
  const classNames = node.properties?.className;
  if (Array.isArray(classNames)) {
    node.properties!.className = classNames.map((className) => {
      if (typeof className !== "string" || !className.startsWith("language-")) {
        return className;
      }
      const canonical = normalizeCodeLanguageLabel(className.slice(9));
      return canonical ? `language-${canonical}` : className;
    });
  }
  for (const child of node.children ?? []) {
    visitHastNodes(child);
  }
}
