import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

interface FeasibilityReport {
  boundedRemediation: string[];
  budgets: Record<string, { passed: boolean }>;
  bundle: {
    codeGzipBytes: number;
    codeRawBytes: number;
    currentProductionEntryImpactBytes: number;
  };
  packageEvidence: {
    excludedPackagePrefixes: string[];
    included: Array<{ license: string; name: string; version: string }>;
  };
  schemaVersion: number;
  sourceRevision: string;
  spike: string;
}

const report = JSON.parse(
  readFileSync("fixtures/redesign/blocknote-feasibility-v1.performance.json", "utf8"),
) as FeasibilityReport;

describe("checked-in BlockNote feasibility evidence", () => {
  it("binds open-source packages and passing provisional budgets to a measured revision", () => {
    expect(report.schemaVersion).toBe(1);
    expect(report.spike).toBe("restricted-blocknote-browser-feasibility");
    expect(report.sourceRevision).toMatch(/^[0-9a-f]{40}$/);
    expect(Object.values(report.budgets)).not.toHaveLength(0);
    expect(Object.values(report.budgets).every((budget) => budget.passed)).toBe(true);

    expect(report.packageEvidence.included).toEqual([
      { name: "@blocknote/core", version: "0.52.1", license: "MPL-2.0" },
      { name: "@blocknote/react", version: "0.52.1", license: "MPL-2.0" },
      { name: "@blocknote/mantine", version: "0.52.1", license: "MPL-2.0" },
    ]);
    expect(report.packageEvidence.excludedPackagePrefixes).toEqual(["@blocknote/xl-"]);
  });

  it("keeps the spike isolated while recording its bounded bundle cost", () => {
    expect(report.bundle.currentProductionEntryImpactBytes).toBe(0);
    expect(report.bundle.codeRawBytes).toBeGreaterThan(0);
    expect(report.bundle.codeGzipBytes).toBeGreaterThan(0);
    expect(report.bundle.codeGzipBytes).toBeLessThan(report.bundle.codeRawBytes);
    expect(report.boundedRemediation.length).toBeGreaterThan(0);
  });
});
