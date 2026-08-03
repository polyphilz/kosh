import { expect, test } from "vitest";
// @ts-expect-error The production helper is intentionally plain Node ESM.
import {
  assessPerformanceBudgets,
  referenceHardwareMatches,
} from "../scripts/performance-budgets.mjs";

const reference = {
  platform: "darwin",
  cpu: "Apple M1 Max",
  logicalCpuCount: 10,
  totalMemoryBytes: 68_719_476_736,
};

test("matches only the frozen reference hardware", () => {
  expect(referenceHardwareMatches(reference, reference)).toBe(true);
  expect(referenceHardwareMatches({ ...reference, cpu: "Apple M4" }, reference)).toBe(false);
  expect(
    referenceHardwareMatches({ ...reference, totalMemoryBytes: 34_359_738_368 }, reference),
  ).toBe(false);
});

test("records machine timings without asserting them on unlike hardware", () => {
  const budgets = assessPerformanceBudgets(
    interactive(10_000),
    nativeStartup(10_000),
    lexicalScale(50),
    frozenBaseline(),
    false,
  );

  expect(budgets.hiddenNativeStartupP95).toMatchObject({ enforced: false, passed: null });
  expect(budgets.coldShellP95).toMatchObject({ enforced: false, passed: null });
  expect(budgets.editorInitializationP95).toMatchObject({ enforced: false, passed: null });
  expect(budgets.inputPaintP95).toMatchObject({ enforced: false, passed: null });
  expect(budgets.searchOverlayP95).toMatchObject({ enforced: false, passed: null });
  expect(budgets.firstSearchResultP95).toMatchObject({ enforced: false, passed: null });
  expect(budgets.lexicalScaleP95).toMatchObject({ enforced: true, passed: true });
});

test("fails machine timings on the reference hardware", () => {
  const budgets = assessPerformanceBudgets(
    interactive(10_000),
    nativeStartup(10_000),
    lexicalScale(101),
    frozenBaseline(),
    true,
  );

  expect(Object.values(budgets).every((budget) => budget.enforced)).toBe(true);
  expect(Object.values(budgets).every((budget) => budget.passed === false)).toBe(true);
});

function timing(p95: number) {
  return { p95 };
}

function interactive(p95: number) {
  return {
    coldShellMs: timing(p95),
    editorInitializationMs: timing(p95),
    inputPaintMs: timing(p95),
    searchNavigationMs: timing(p95),
    firstSearchResultMs: timing(p95),
  };
}

function nativeStartup(p95: number) {
  return { freshHiddenProcessMs: timing(p95) };
}

function lexicalScale(queryP95Ms: number) {
  return { queryP95Ms, interactiveP95BudgetMs: 100 };
}

function frozenBaseline() {
  return {
    interactive: interactive(10),
    nativeStartup: { coldProcessMs: timing(10) },
  };
}
