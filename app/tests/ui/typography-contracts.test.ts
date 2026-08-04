import { describe, expect, test } from "vitest";
import {
  findTypographyViolations,
  findTypographyViolationsInSources,
  TypographyCheckKind,
} from "../../scripts/check-typography-contracts.mjs";

const cssViolations = (contents: string) => findTypographyViolations("src/feature.css", contents);

describe("typography contract", () => {
  test("rejects raw CSS sizes, weights, leading, and tracking", () => {
    const found = cssViolations(`
      .copy {
        font-size: 12px;
        font-weight: 700;
        line-height: 1.4;
        letter-spacing: 0.02em;
      }
    `);

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.FontSize,
      TypographyCheckKind.FontWeight,
      TypographyCheckKind.LineHeight,
      TypographyCheckKind.LetterSpacing,
    ]);
  });

  test("rejects raw typography values nested inside CSS functions", () => {
    const found = cssViolations(`
      .copy {
        font-weight: calc(var(--type-weight-body) + 100);
        line-height: var(--type-leading-body, 1.4);
        letter-spacing: var(--type-tracking-body, -0.02em);
      }
    `);

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.FontWeight,
      TypographyCheckKind.LineHeight,
      TypographyCheckKind.LetterSpacing,
    ]);
  });

  test("rejects scientific notation in CSS and inline typography values", () => {
    const cssFound = cssViolations(`
      .copy {
        font-size: 1e2px;
        font-weight: 7e2;
        line-height: 1.4e0;
        letter-spacing: 2e-2em;
      }
    `);
    const inlineFound = findTypographyViolations(
      "src/Feature.tsx",
      `const style = { fontSize: "1e2px", fontWeight: 7e2, lineHeight: 1.4e0 };`,
    );

    expect(cssFound.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.FontSize,
      TypographyCheckKind.FontWeight,
      TypographyCheckKind.LineHeight,
      TypographyCheckKind.LetterSpacing,
    ]);
    expect(inlineFound.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.InlineFontSize,
      TypographyCheckKind.FontWeight,
      TypographyCheckKind.LineHeight,
    ]);
  });

  test("does not mistake numbers in token names for raw typography values", () => {
    expect(
      cssViolations(`
        .copy {
          font-weight: var(--type-weight-2xl);
          line-height: var(--type-leading-2xl);
          letter-spacing: var(--type-tracking-2xl);
        }
      `),
    ).toEqual([]);
  });

  test("rejects multiline font shorthand and relative sizes", () => {
    const found = cssViolations(`
      .copy {
        font:
          650 12px/1.4 monospace;
        font-size:
          1.2em;
      }
    `);

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.FontShorthand,
      TypographyCheckKind.FontSize,
    ]);
  });

  test("rejects raw weight and leading inside font shorthand", () => {
    const found = cssViolations(`
      .copy {
        font: calc(600 + 100) var(--type-size-body)/calc(1 + .4)
          var(--font-family-app);
      }
      .valid {
        font: var(--type-weight-body) var(--type-size-body)/var(--type-leading-body)
          var(--font-family-app);
      }
    `);

    expect(found.map((entry) => entry.check)).toEqual([TypographyCheckKind.FontShorthand]);
  });

  test("matches CSS typography properties case-insensitively", () => {
    const found = cssViolations(`
      .copy {
        FONT-SIZE: 12px;
        Font-Weight: 700;
        LINE-HEIGHT: 1.4;
      }
    `);

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.FontSize,
      TypographyCheckKind.FontWeight,
      TypographyCheckKind.LineHeight,
    ]);
  });

  test("rejects unitless zero font sizes", () => {
    const found = cssViolations(`
      .hidden { font-size: 0; }
      .collapsed { font: 0 monospace; }
    `);

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.FontSize,
      TypographyCheckKind.FontShorthand,
    ]);
  });

  test("rejects raw inline style values", () => {
    const found = findTypographyViolations(
      "src/Feature.tsx",
      `const style = { fontSize: compact ? 12 : 14, fontWeight: 700 };
       const node = <text lineHeight={1.2} letterSpacing={0.02} />;`,
    );

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.InlineFontSize,
      TypographyCheckKind.FontWeight,
      TypographyCheckKind.LineHeight,
      TypographyCheckKind.LetterSpacing,
    ]);
  });

  test("checks complete inline fallbacks without rejecting numbered tokens", () => {
    const found = findTypographyViolations(
      "src/Feature.tsx",
      `const valid = { fontSize: "var(--type-size-content-heading-1)" };
       const invalid = {
         fontSize: "var(--type-size-body, 12px)",
         fontWeight: "var(--type-weight-body, 700)",
       };`,
    );

    expect(found.map((entry) => entry.check)).toEqual([
      TypographyCheckKind.InlineFontSize,
      TypographyCheckKind.FontWeight,
    ]);
  });

  test("rejects feature variables that hide raw typography", () => {
    const found = cssViolations(`
      :root { --feature-copy: 12px; }
      .copy { font-size: var(--feature-copy); }
    `);

    expect(found).toHaveLength(1);
    expect(found[0].check).toBe(TypographyCheckKind.TypographyVariable);
  });

  test("resolves cross-file variables and preserves repeated definitions", () => {
    const found = findTypographyViolationsInSources(
      [
        {
          path: "src/typography.css",
          contents: ":root { --type-size-body: 1rem; }",
        },
        {
          path: "src/feature-tokens.css",
          contents: `
            :root { --feature-copy: 12px; }
            @media (min-width: 800px) { :root { --feature-copy: 14px; } }
          `,
        },
        {
          path: "src/feature.css",
          contents: ".copy { font-size: var(--feature-copy); }",
        },
      ],
      "src/typography.css",
    );

    expect(found).toHaveLength(2);
    expect(found.map((entry) => entry.path)).toEqual([
      "src/feature-tokens.css",
      "src/feature-tokens.css",
    ]);
    expect(found.map((entry) => entry.line)).toEqual([2, 3]);
  });

  test("resolves typography variables referenced by inline styles", () => {
    const found = findTypographyViolationsInSources(
      [
        {
          path: "src/typography.css",
          contents: ":root { --type-size-body: 1rem; }",
        },
        {
          path: "src/feature-tokens.css",
          contents:
            ":root { --feature-copy: var(--feature-copy-base); --feature-copy-base: 12px; }",
        },
        {
          path: "src/Feature.tsx",
          contents: `const style = { fontSize: "var(--feature-copy)" };`,
        },
      ],
      "src/typography.css",
    );

    expect(found).toHaveLength(1);
    expect(found[0]).toMatchObject({
      check: TypographyCheckKind.TypographyVariable,
      path: "src/feature-tokens.css",
    });
  });

  test("accepts centralized token references and inheritance", () => {
    expect(
      cssViolations(`
        .copy {
          font-size: var(--type-size-body);
          font-weight: var(--type-weight-body);
          line-height: var(--type-leading-body);
          letter-spacing: var(--type-tracking-body);
        }
        button { font: inherit; }
      `),
    ).toEqual([]);
  });

  test("ignores comments and unrelated geometry", () => {
    expect(
      cssViolations(`
        /* font-size: 92px; */
        .icon { width: 12px; height: 12px; }
      `),
    ).toEqual([]);
  });
});
