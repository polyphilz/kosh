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

const rawTypographyNumericPattern =
  /(?<![\w$-])-?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?(?:[a-z%]+)?(?![\w$-])/i;
const typographyProperties = ["font-size", "font", "font-weight", "letter-spacing", "line-height"];

function stripComments(contents, lineComments) {
  let analyzed = "";
  let quote = null;
  let escaped = false;
  for (let index = 0; index < contents.length; index += 1) {
    const character = contents[index];
    const next = contents[index + 1];
    if (quote !== null) {
      analyzed += character;
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === '"' || character === "'" || character === "`") {
      quote = character;
      analyzed += character;
      continue;
    }
    if (character === "/" && next === "*") {
      analyzed += "  ";
      index += 2;
      while (index < contents.length && !(contents[index] === "*" && contents[index + 1] === "/")) {
        analyzed += contents[index] === "\n" ? "\n" : " ";
        index += 1;
      }
      if (index < contents.length) {
        analyzed += "  ";
        index += 1;
      }
      continue;
    }
    if (lineComments && character === "/" && next === "/") {
      analyzed += "  ";
      index += 2;
      while (index < contents.length && contents[index] !== "\n") {
        analyzed += " ";
        index += 1;
      }
      if (index < contents.length) analyzed += "\n";
      continue;
    }
    analyzed += character;
  }
  return analyzed;
}

function lineAt(contents, offset) {
  return contents.slice(0, offset).split("\n").length;
}

function sourceAt(contents, line) {
  return contents.split("\n")[line - 1]?.trim() ?? "";
}

function cssCheck(property, value) {
  if (approvedCssTypographyValue(value)) return null;
  if (property === "font-size") {
    return [TypographyCheckKind.FontSize, "font-size uses an untokenized typography value"];
  }
  if (property === "font") {
    return [
      TypographyCheckKind.FontShorthand,
      "font shorthand embeds an untokenized typography value",
    ];
  }
  if (property === "font-weight") {
    return [TypographyCheckKind.FontWeight, "font-weight uses an untokenized typography value"];
  }
  if (property === "letter-spacing") {
    return [
      TypographyCheckKind.LetterSpacing,
      "letter-spacing uses an untokenized typography value",
    ];
  }
  return [TypographyCheckKind.LineHeight, "line-height uses an untokenized typography value"];
}

function approvedCssTypographyValue(value) {
  const candidate = value.replace(/\s*!important\s*$/i, "").trim();
  if (/^(?:inherit|initial|revert|revert-layer|unset)$/i.test(candidate)) return true;
  const withoutTokens = candidate.replace(/var\(\s*--[\w-]+\s*\)/gi, "");
  return withoutTokens !== candidate && /^[\s/]*$/.test(withoutTokens);
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

function referencedTypographyVariables(value) {
  return [...value.matchAll(/var\(\s*(--[\w-]+)/gi)].map((entry) => entry[1]);
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
    referencedVariables.push(...referencedTypographyVariables(value));
  }

  const visited = new Set();
  while (referencedVariables.length > 0) {
    const variable = referencedVariables.pop();
    if (!variable) continue;
    for (const definition of definitions.get(variable) ?? []) {
      const identity = `${definition.path}:${definition.line}:${variable}`;
      if (visited.has(identity)) continue;
      visited.add(identity);
      referencedVariables.push(...referencedTypographyVariables(definition.value));
      if (definition.path !== allowedTokenSource && !approvedCssTypographyValue(definition.value)) {
        violations.push({
          path: definition.path,
          line: definition.line,
          check: TypographyCheckKind.TypographyVariable,
          message: "a typography variable hides an untokenized value outside the token source",
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
    font: TypographyCheckKind.FontShorthand,
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
    const staticValue = staticInlineCssValue(value);
    if (
      rawTypographyNumericPattern.test(value) ||
      staticValue === null ||
      !approvedInlineCssValue(staticValue)
    ) {
      violations.push({
        path,
        line,
        check: properties[property],
        message: `${property} must use a static type token or inheritance value`,
        source: sourceAt(raw, line),
      });
    }
    referencedVariables.push(...referencedTypographyVariables(value));
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
      referencedVariables.push(...referencedTypographyVariables(definition.value));
      if (definition.path !== allowedTokenSource && !approvedCssTypographyValue(definition.value)) {
        violations.push({
          path: definition.path,
          line: definition.line,
          check: TypographyCheckKind.TypographyVariable,
          message: "a typography variable hides an untokenized value outside the token source",
          source: sourceAt(definition.raw, definition.line),
        });
      }
    }
  }
  return violations;
}

function staticInlineCssValue(value) {
  let candidate = value.trim();
  if (candidate.startsWith("{") && candidate.endsWith("}")) {
    candidate = candidate.slice(1, -1).trim();
  }
  const quote = candidate[0];
  if (
    (quote !== '"' && quote !== "'" && quote !== "`") ||
    candidate.at(-1) !== quote ||
    (quote === "`" && candidate.includes("${"))
  ) {
    return null;
  }
  return candidate.slice(1, -1).trim();
}

function approvedInlineCssValue(value) {
  if (/^(?:inherit|initial|revert|revert-layer|unset)$/i.test(value)) return true;
  const withoutTokens = value.replace(/var\(\s*--[\w-]+\s*\)/gi, "");
  return withoutTokens !== value && /^[\s/]*$/.test(withoutTokens);
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
  const violations = analyzedSources.flatMap((source) =>
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
