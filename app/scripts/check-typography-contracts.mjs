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

const sizedNumericPattern = /(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?\s*(?:%|[a-z]+)(?![\w-])/i;
const unitlessZeroPattern = /(^|[^\w$.-])-?0(?:\.0*)?(?:e[+-]?\d+)?(?![\w$.-])/i;
const rawTypographyNumericPattern =
  /(?<![\w$-])-?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?(?:[a-z%]+)?(?![\w$-])/i;
const typographyProperties = ["font-size", "font", "font-weight", "letter-spacing", "line-height"];

function stripComments(contents, lineComments) {
  const withoutBlocks = contents.replace(/\/\*[\s\S]*?\*\//g, (comment) =>
    comment.replace(/[^\n]/g, " "),
  );
  return lineComments ? withoutBlocks.replace(/(^|[^:])\/\/.*$/gm, "$1") : withoutBlocks;
}

function lineAt(contents, offset) {
  return contents.slice(0, offset).split("\n").length;
}

function sourceAt(contents, line) {
  return contents.split("\n")[line - 1]?.trim() ?? "";
}

function cssCheck(property, value) {
  if (property === "font-size") {
    return sizedNumericPattern.test(value) || unitlessZeroPattern.test(value)
      ? [TypographyCheckKind.FontSize, "font-size uses a raw size instead of a type token"]
      : null;
  }
  if (property === "font") {
    return rawTypographyNumericPattern.test(value)
      ? [
          TypographyCheckKind.FontShorthand,
          "font shorthand embeds a raw typography value instead of separate type tokens",
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
    "gim",
  );
  const definitions = sharedDefinitions ?? cssDefinitions(path, raw, analyzed);

  const referencedVariables = [];
  for (const match of analyzed.matchAll(declarations)) {
    const property = match[1].toLowerCase();
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
      if (
        definition.path !== allowedTokenSource &&
        rawTypographyNumericPattern.test(definition.value)
      ) {
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

function inlineViolations(path, raw, analyzed, definitions, allowedTokenSource) {
  const violations = [];
  const properties = {
    fontSize: TypographyCheckKind.InlineFontSize,
    fontWeight: TypographyCheckKind.FontWeight,
    letterSpacing: TypographyCheckKind.LetterSpacing,
    lineHeight: TypographyCheckKind.LineHeight,
  };
  const propertyNames = Object.keys(properties).join("|");
  const assignments = new RegExp(
    `(?:(["'])(${propertyNames})\\1|\\b(${propertyNames})\\b)\\s*(?::|=)\\s*`,
    "g",
  );
  const referencedVariables = [];

  for (const match of analyzed.matchAll(assignments)) {
    const value = readInlineValue(analyzed, match.index + match[0].length);
    const property = match[2] ?? match[3];
    const line = lineAt(analyzed, match.index);
    if (rawTypographyNumericPattern.test(value)) {
      violations.push({
        path,
        line,
        check: properties[property],
        message: `${property} uses a raw numeric value instead of a type token`,
        source: sourceAt(raw, line),
      });
    }
    referencedVariables.push(...value.matchAll(/var\(\s*(--[\w-]+)/g).map((entry) => entry[1]));
  }

  if (!definitions) return violations;
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
      if (
        definition.path !== allowedTokenSource &&
        rawTypographyNumericPattern.test(definition.value)
      ) {
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

function readInlineValue(contents, start) {
  const first = contents[start];
  const wrappedByQuote = first === '"' || first === "'" || first === "`" ? first : null;
  const wrappedByBrace = first === "{";
  let quote = null;
  let escaped = false;
  let roundDepth = 0;
  let squareDepth = 0;
  let curlyDepth = 0;

  for (let index = start; index < contents.length; index += 1) {
    const character = contents[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = null;
        if (wrappedByQuote === character) return contents.slice(start, index + 1);
      }
      continue;
    }
    if (character === '"' || character === "'" || character === "`") {
      quote = character;
      continue;
    }
    if (character === "(") roundDepth += 1;
    else if (character === ")" && roundDepth > 0) roundDepth -= 1;
    else if (character === "[") squareDepth += 1;
    else if (character === "]" && squareDepth > 0) squareDepth -= 1;
    else if (character === "{") curlyDepth += 1;
    else if (character === "}" && curlyDepth > 0) {
      curlyDepth -= 1;
      if (wrappedByBrace && curlyDepth === 0) return contents.slice(start, index + 1);
    } else if (
      roundDepth === 0 &&
      squareDepth === 0 &&
      curlyDepth === 0 &&
      (character === "," || character === ";" || character === "\n" || character === ">")
    ) {
      return contents.slice(start, index);
    }
  }
  return contents.slice(start);
}

export function findTypographyViolations(path, contents) {
  const css = extname(path) === ".css";
  const analyzed = stripComments(contents, !css);
  return css
    ? cssViolations(path, contents, analyzed, undefined, undefined)
    : inlineViolations(path, contents, analyzed);
}

export function findTypographyViolationsInSources(sources, allowedTokenSource) {
  const analyzedSources = sources.map(({ path, contents }) => ({
    analyzed: stripComments(contents, extname(path) !== ".css"),
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
        : inlineViolations(
            source.path,
            source.contents,
            source.analyzed,
            definitions,
            allowedTokenSource,
          ),
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
