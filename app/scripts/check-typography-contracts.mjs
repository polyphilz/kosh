import { readFileSync, readdirSync } from "node:fs";
import { extname, relative, resolve } from "node:path";

const sourceRoot = resolve("src");
const tokenSource = resolve("src/typography.css");
const checkedExtensions = new Set([".css", ".ts", ".tsx"]);

export const TypographyCheckKind = {
  FontSize: "font-size",
  FontShorthand: "font shorthand",
  InlineFontSize: "fontSize",
  FontWeight: "font-weight",
  LetterSpacing: "letter-spacing",
  LineHeight: "line-height",
  TypographyVariable: "typography variable",
};

const numericPattern = /(^|[^\w$])(?:\d+(?:\.\d*)?|\.\d+)(?:[a-z%]+)?(?![\w$])/i;
const sizedNumericPattern = /(?:\d+(?:\.\d*)?|\.\d+)\s*(?:%|[a-z]+)(?![\w-])/i;
const rawTypographyNumericPattern = /(?<![\w$-])-?(?:\d+(?:\.\d*)?|\.\d+)(?:[a-z%]+)?(?![\w$-])/i;
const typographyProperties = ["font-size", "font", "font-weight", "letter-spacing", "line-height"];

function stripComments(contents) {
  return contents
    .replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "))
    .replace(/(^|[^:])\/\/.*$/gm, "$1");
}

function lineAt(contents, offset) {
  return contents.slice(0, offset).split("\n").length;
}

function sourceAt(contents, line) {
  return contents.split("\n")[line - 1]?.trim() ?? "";
}

function cssCheck(property, value) {
  if (property === "font-size") {
    return sizedNumericPattern.test(value)
      ? [TypographyCheckKind.FontSize, "font-size uses a raw size instead of a type token"]
      : null;
  }
  if (property === "font") {
    return sizedNumericPattern.test(value)
      ? [
          TypographyCheckKind.FontShorthand,
          "font shorthand embeds a raw size instead of separate type tokens",
        ]
      : null;
  }
  if (property === "font-weight") {
    return rawTypographyNumericPattern.test(value)
      ? [TypographyCheckKind.FontWeight, "font-weight uses a raw number instead of a type token"]
      : null;
  }
  if (property === "letter-spacing") {
    return rawTypographyNumericPattern.test(value)
      ? [
          TypographyCheckKind.LetterSpacing,
          "letter-spacing uses a raw number instead of a type token",
        ]
      : null;
  }
  return rawTypographyNumericPattern.test(value)
    ? [TypographyCheckKind.LineHeight, "line-height uses a raw number instead of a type token"]
    : null;
}

function cssDefinitions(path, raw, analyzed) {
  const definitions = new Map();
  const definitionPattern = /(?:^|[;{])\s*(--[\w-]+)\s*:\s*([^;}]+)/gm;
  for (const match of analyzed.matchAll(definitionPattern)) {
    const name = match[1];
    const definition = {
      line: lineAt(analyzed, match.index + match[0].indexOf(name)),
      path,
      raw,
      value: match[2],
    };
    const matches = definitions.get(name) ?? [];
    matches.push(definition);
    definitions.set(name, matches);
  }
  return definitions;
}

function mergeDefinitions(target, source) {
  for (const [name, definitions] of source) {
    target.set(name, [...(target.get(name) ?? []), ...definitions]);
  }
  return target;
}

function cssViolations(path, raw, analyzed, sharedDefinitions, allowedTokenSource) {
  const violations = [];
  const declarations = new RegExp(
    `(?:^|[;{])\\s*(${typographyProperties.join("|")})\\s*:\\s*([^;}]+)`,
    "gm",
  );
  const definitions = sharedDefinitions ?? cssDefinitions(path, raw, analyzed);

  const referencedVariables = [];
  for (const match of analyzed.matchAll(declarations)) {
    const property = match[1];
    const value = match[2];
    const line = lineAt(analyzed, match.index + match[0].indexOf(property));
    const failed = cssCheck(property, value);
    if (failed) {
      violations.push({
        path,
        line,
        check: failed[0],
        message: failed[1],
        source: sourceAt(raw, line),
      });
    }
    referencedVariables.push(...value.matchAll(/var\(\s*(--[\w-]+)/g).map((entry) => entry[1]));
  }

  const visited = new Set();
  while (referencedVariables.length > 0) {
    const variable = referencedVariables.pop();
    if (!variable) continue;
    for (const definition of definitions.get(variable) ?? []) {
      const identity = `${definition.path}:${definition.line}:${variable}`;
      if (visited.has(identity)) continue;
      visited.add(identity);
      referencedVariables.push(
        ...definition.value.matchAll(/var\(\s*(--[\w-]+)/g).map((entry) => entry[1]),
      );
      if (definition.path !== allowedTokenSource && numericPattern.test(definition.value)) {
        violations.push({
          path: definition.path,
          line: definition.line,
          check: TypographyCheckKind.TypographyVariable,
          message: "a typography variable hides a raw numeric value outside the token source",
          source: sourceAt(definition.raw, definition.line),
        });
      }
    }
  }

  return violations;
}

function inlineViolations(path, raw, analyzed) {
  const violations = [];
  const properties = {
    fontSize: TypographyCheckKind.InlineFontSize,
    fontWeight: TypographyCheckKind.FontWeight,
    letterSpacing: TypographyCheckKind.LetterSpacing,
    lineHeight: TypographyCheckKind.LineHeight,
  };
  const assignments = new RegExp(
    `\\b(${Object.keys(properties).join("|")})\\b\\s*(?::|=\\s*\\{?)\\s*([\\s\\S]*?)(?=,|;|\\}|\\n\\s*[A-Za-z_$][\\w$]*\\s*:|$)`,
    "g",
  );

  for (const match of analyzed.matchAll(assignments)) {
    if (!numericPattern.test(match[2])) continue;
    const property = match[1];
    const line = lineAt(analyzed, match.index);
    violations.push({
      path,
      line,
      check: properties[property],
      message: `${property} uses a raw numeric value instead of a type token`,
      source: sourceAt(raw, line),
    });
  }
  return violations;
}

export function findTypographyViolations(path, contents) {
  const analyzed = stripComments(contents);
  return extname(path) === ".css"
    ? cssViolations(path, contents, analyzed, undefined, undefined)
    : inlineViolations(path, contents, analyzed);
}

export function findTypographyViolationsInSources(sources, allowedTokenSource) {
  const analyzedSources = sources.map(({ path, contents }) => ({
    analyzed: stripComments(contents),
    contents,
    path,
  }));
  const definitions = analyzedSources
    .filter(({ path }) => extname(path) === ".css")
    .reduce(
      (all, source) =>
        mergeDefinitions(all, cssDefinitions(source.path, source.contents, source.analyzed)),
      new Map(),
    );
  const violations = analyzedSources
    .filter(({ path }) => path !== allowedTokenSource)
    .flatMap((source) =>
      extname(source.path) === ".css"
        ? cssViolations(
            source.path,
            source.contents,
            source.analyzed,
            definitions,
            allowedTokenSource,
          )
        : inlineViolations(source.path, source.contents, source.analyzed),
    );
  return [
    ...new Map(
      violations.map((violation) => [
        `${violation.path}:${violation.line}:${violation.check}`,
        violation,
      ]),
    ).values(),
  ];
}

function listSourceFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) return listSourceFiles(path);
    return checkedExtensions.has(extname(entry.name)) ? [path] : [];
  });
}

function main() {
  const sources = listSourceFiles(sourceRoot).map((path) => ({
    contents: readFileSync(path, "utf8"),
    path,
  }));
  const violations = findTypographyViolationsInSources(sources, tokenSource);

  if (violations.length > 0) {
    for (const violation of violations) {
      console.error(
        `${relative(process.cwd(), violation.path)}:${violation.line}  ${violation.message}\n    ${violation.source}`,
      );
    }
    console.error(
      `\nUse KoshText or the tokens in src/typography.css. ${violations.length} violation(s).`,
    );
    process.exit(1);
  }

  console.log(`Typography contracts passed: ${sources.length - 1} source files checked.`);
}

if (import.meta.url === `file://${process.argv[1]}`) main();
