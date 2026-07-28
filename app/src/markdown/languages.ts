interface CodeLanguageDefinition {
  aliases: readonly string[];
  canonical: string;
}

export const codeLanguageDefinitions = [
  { canonical: "bash", aliases: ["bash", "sh", "shell", "zsh"] },
  { canonical: "c", aliases: ["c"] },
  { canonical: "cpp", aliases: ["cpp", "c++"] },
  { canonical: "css", aliases: ["css"] },
  { canonical: "go", aliases: ["go"] },
  { canonical: "html", aliases: ["html", "xhtml"] },
  { canonical: "xml", aliases: ["xml"] },
  { canonical: "java", aliases: ["java"] },
  { canonical: "kotlin", aliases: ["kotlin", "kt"] },
  { canonical: "javascript", aliases: ["javascript", "js"] },
  { canonical: "jsx", aliases: ["jsx"] },
  { canonical: "typescript", aliases: ["typescript", "ts"] },
  { canonical: "tsx", aliases: ["tsx"] },
  { canonical: "json", aliases: ["json"] },
  { canonical: "markdown", aliases: ["markdown", "md"] },
  { canonical: "python", aliases: ["python", "py"] },
  { canonical: "rust", aliases: ["rust", "rs"] },
  { canonical: "sql", aliases: ["sql"] },
  { canonical: "swift", aliases: ["swift"] },
  { canonical: "toml", aliases: ["toml"] },
  { canonical: "yaml", aliases: ["yaml", "yml"] },
] as const satisfies readonly CodeLanguageDefinition[];

const normalizedLanguages = new Map<string, string>();
const languageDisplayNames: Readonly<Record<string, string>> = {
  bash: "Bash",
  c: "C",
  cpp: "C++",
  css: "CSS",
  go: "Go",
  html: "HTML",
  java: "Java",
  javascript: "JavaScript",
  json: "JSON",
  jsx: "JSX",
  kotlin: "Kotlin",
  markdown: "Markdown",
  python: "Python",
  rust: "Rust",
  sql: "SQL",
  swift: "Swift",
  toml: "TOML",
  tsx: "TSX",
  typescript: "TypeScript",
  xml: "XML",
  yaml: "YAML",
};

for (const definition of codeLanguageDefinitions) {
  normalizedLanguages.set(definition.canonical, definition.canonical);
  for (const alias of definition.aliases) {
    normalizedLanguages.set(alias, definition.canonical);
  }
}

export function normalizeCodeLanguageLabel(label: string): string | null {
  const name = label.trim().split(/\s+/, 1)[0]?.toLowerCase();
  return name ? (normalizedLanguages.get(name) ?? null) : null;
}

export function codeLanguageDisplayName(language: string | null): string {
  return language ? (languageDisplayNames[language] ?? language) : "Plain code";
}

export const codeLanguageAliases = Object.fromEntries(
  codeLanguageDefinitions.map((definition) => [
    definition.canonical,
    definition.aliases.filter((alias) => alias !== definition.canonical),
  ]),
) as Readonly<Record<string, readonly string[]>>;
