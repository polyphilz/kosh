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
