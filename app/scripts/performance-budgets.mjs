export function referenceHardwareMatches(actual, reference) {
  return (
    actual.platform === reference.platform &&
    actual.cpu === reference.cpu &&
    actual.logicalCpuCount === reference.logicalCpuCount &&
    actual.totalMemoryBytes === reference.totalMemoryBytes
  );
}

export function assessPerformanceBudgets(
  interactive,
  nativeStartup,
  scale,
  frozenBaseline,
  enforceMachineTimings,
) {
  const frozen = frozenBaseline.interactive;
  const machineBudget = (actual, limit, rationale) =>
    budget(actual, limit, rationale, enforceMachineTimings);
  return {
    hiddenNativeStartupP95: machineBudget(
      nativeStartup.freshHiddenProcessMs.p95,
      round(frozenBaseline.nativeStartup.coldProcessMs.p95 * 1.2),
      "hidden exact-head startup regression evidence; visible focus is measured manually",
    ),
    coldShellP95: machineBudget(
      interactive.coldShellMs.p95,
      round(frozen.coldShellMs.p95 * 1.2),
      "within 20% of the frozen shell baseline",
    ),
    editorInitializationP95: machineBudget(
      interactive.editorInitializationMs.p95,
      round(frozen.editorInitializationMs.p95 * 1.3),
      "explicitly reviewed BlockNote ceiling of 30% over the frozen ProseMirror baseline",
    ),
    inputPaintP95: machineBudget(
      interactive.inputPaintMs.p95,
      16.67,
      "ordinary input paints within one 60 Hz frame",
    ),
    searchOverlayP95: machineBudget(
      interactive.searchNavigationMs.p95,
      100,
      "warm Command-K overlay",
    ),
    firstSearchResultP95: machineBudget(
      interactive.firstSearchResultMs.p95,
      round(frozen.firstSearchResultMs.p95 * 1.2),
      "within 20% of the frozen deterministic result baseline",
    ),
    lexicalScaleP95: budget(
      scale.queryP95Ms,
      scale.interactiveP95BudgetMs,
      "10,000-note production lexical path",
      true,
    ),
  };
}

function budget(actual, limit, rationale, enforced) {
  return {
    actual,
    limit,
    enforced,
    passed: enforced ? actual <= limit : null,
    rationale: enforced
      ? rationale
      : `${rationale}; recorded only because this is not the frozen reference hardware`,
  };
}

function round(value) {
  return Math.round(value * 100) / 100;
}
