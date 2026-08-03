import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { cpus, platform, release, tmpdir, totalmem } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { writePrivateReport } from "./private-report-output.mjs";

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(appRoot, "..");
const outputPath = resolve(
  appRoot,
  process.argv[2] ?? ".data/redesign/release-candidate-v1.performance.json",
);
const frozenBaselinePath = join(appRoot, "fixtures/redesign/baseline-v1.performance.json");
const scaleReportPath = join(appRoot, ".data/relevance/reports/lexical-scale-v1.performance.json");
const sampleCount = 20;
const nativeSampleCount = 20;
const baseUrl = "http://127.0.0.1:1422";

const sourceRevision = (await commandOutput("git", ["rev-parse", "HEAD"], repositoryRoot)).trim();
const buildRevision = sourceRevision;
const worktreeStatus = await commandOutput(
  "git",
  ["status", "--porcelain=v1", "--untracked-files=all"],
  repositoryRoot,
);
if (worktreeStatus.trim().length > 0) {
  throw new Error(
    `baseline recording requires a clean HEAD; commit or stash these changes first:\n${worktreeStatus}`,
  );
}
const scale = JSON.parse(await readFile(scaleReportPath, "utf8"));
const frozenBaseline = JSON.parse(await readFile(frozenBaselinePath, "utf8"));
const server = spawn(
  process.execPath,
  [
    join(appRoot, "node_modules/vite/bin/vite.js"),
    "--host",
    "127.0.0.1",
    "--port",
    "1422",
    "--strictPort",
  ],
  {
    cwd: appRoot,
    env: { ...process.env, NO_COLOR: "1", VITE_KOSH_BACKEND: "fake" },
    stdio: ["ignore", "pipe", "pipe"],
  },
);
let serverOutput = "";
server.stdout.on("data", (chunk) => {
  serverOutput += chunk.toString();
});
server.stderr.on("data", (chunk) => {
  serverOutput += chunk.toString();
});

try {
  await waitForServer(`${baseUrl}/`);
  const browser = await chromium.launch({ headless: true });
  try {
    const coldShellMs = [];
    const editorInitializationMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      coldShellMs.push(await measureColdNote(browser));
      editorInitializationMs.push(await measureEditorInitialization(browser));
    }

    const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
    const page = await context.newPage();
    await page.goto(`${baseUrl}/#/`);
    const editor = page.getByRole("textbox", { name: "Note" });
    await editor.waitFor({ state: "visible" });
    await editor.focus();
    await editor.press("x");
    await editor.press("Backspace");
    const inputPaintMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      const painted = measureNextInputPaint(page);
      await editor.press(index % 2 === 0 ? "x" : "Backspace");
      inputPaintMs.push(await painted);
    }
    await context.close();

    const searchNavigationMs = [];
    const firstSearchResultMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      const searchContext = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
      const searchPage = await searchContext.newPage();
      await searchPage.goto(`${baseUrl}/#/`);
      searchNavigationMs.push(await measureSearchNavigation(searchPage));
      firstSearchResultMs.push(await measureFirstSearchResult(searchPage, index));
      await searchContext.close();
    }
    const nativeStartup = await measureNativeStartup();

    const interactive = {
      coldShellMs: summarize(coldShellMs),
      editorInitializationMs: summarize(editorInitializationMs),
      inputPaintMs: summarize(inputPaintMs),
      searchNavigationMs: summarize(searchNavigationMs),
      firstSearchResultMs: summarize(firstSearchResultMs),
    };
    const budgets = assessBudgets(interactive, nativeStartup, scale, frozenBaseline);
    const report = {
      schemaVersion: 1,
      baseline: "note-first-release-candidate",
      sourceRevision,
      recordedAt: new Date().toISOString(),
      environment: {
        platform: platform(),
        release: release(),
        cpu: cpus()[0]?.model ?? "unknown",
        logicalCpuCount: cpus().length,
        totalMemoryBytes: totalmem(),
        browser: await browser.version(),
        node: process.version,
      },
      methodology: {
        samplesPerInteractiveMetric: sampleCount,
        samplesPerNativeMetric: nativeSampleCount,
        browserMode: "headless Chromium against the deterministic fake backend",
        coldShell: "new browser context to the focused titleless note editor",
        editorInitialization: "new browser context to the visible BlockNote editor",
        inputPaint: "beforeinput event through the next animation frame",
        searchNavigation:
          "browser performance time from command dispatch to the focused Command-K overlay",
        firstSearchResult: "Playwright wall time from fill to first deterministic result option",
        lexicalScale: "existing release-mode 10,000-note / 200-query benchmark",
        nativeStartup:
          "hidden process spawn through the complete exact-head Tauri startup-smoke receipt; fresh uses a new profile and restart reopens one preserved profile; this does not measure a shown or focused native window",
      },
      manualMeasurementsRequired: {
        visibleColdLaunch: {
          samples: 20,
          targetP95Ms: 1_000,
          contract: "process launch to a shown native window with a focused editable caret",
        },
        warmWindowReactivation: {
          samples: 20,
          targetP95Ms: 150,
          contract:
            "reactivation of the already-running app with route, selection, and scroll intact",
        },
      },
      interactive,
      nativeStartup,
      lexicalScale: scale,
      budgets,
    };
    await writePrivateReport(
      join(appRoot, ".data/redesign"),
      outputPath,
      `${JSON.stringify(report, null, 2)}\n`,
    );
    process.stdout.write(`Wrote ${outputPath}\n`);
    const failures = Object.entries(budgets)
      .filter(([, value]) => !value.passed)
      .map(([name, value]) => `${name}: ${value.actual} > ${value.limit}`);
    if (failures.length > 0) {
      throw new Error(`redesign performance budgets failed:\n${failures.join("\n")}`);
    }
  } finally {
    await browser.close();
  }
} finally {
  server.kill("SIGTERM");
  await new Promise((resolveExit) => {
    if (server.exitCode !== null) resolveExit();
    else server.once("exit", resolveExit);
  });
}

async function measureColdNote(browser) {
  const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
  const page = await context.newPage();
  const started = performance.now();
  await page.goto(`${baseUrl}/#/`);
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.waitFor({ state: "visible" });
  await editor.focus();
  const duration = performance.now() - started;
  await context.close();
  return duration;
}

async function measureEditorInitialization(browser) {
  const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
  const page = await context.newPage();
  const started = performance.now();
  await page.goto(`${baseUrl}/#/`);
  await page.getByRole("textbox", { name: "Note" }).waitFor({ state: "visible" });
  const duration = performance.now() - started;
  await context.close();
  return duration;
}

async function measureNextInputPaint(page) {
  return page.evaluate(
    () =>
      new Promise((resolvePaint) => {
        const editor = document.querySelector("[role='textbox'][aria-label='Note']");
        if (!editor) throw new Error("editor is unavailable");
        let started = 0;
        editor.addEventListener(
          "beforeinput",
          () => {
            started = performance.now();
          },
          { once: true },
        );
        editor.addEventListener(
          "input",
          () => {
            requestAnimationFrame(() => resolvePaint(performance.now() - started));
          },
          { once: true },
        );
      }),
  );
}

async function measureSearchNavigation(page) {
  await page.evaluate(() => {
    const root = document.documentElement;
    delete root.dataset.koshSearchFocusMs;
    const started = performance.now();
    const recordFocusedOverlay = () => {
      if (document.activeElement?.matches("[data-kosh-search-input]")) {
        root.dataset.koshSearchFocusMs = String(performance.now() - started);
        return;
      }
      requestAnimationFrame(recordFocusedOverlay);
    };
    requestAnimationFrame(recordFocusedOverlay);
  });
  await page.keyboard.press("Meta+k");
  const search = page.getByRole("combobox", { name: "Search notes" });
  await search.waitFor({ state: "visible" });
  await search.focus();
  await page.waitForFunction(
    () => document.documentElement.dataset.koshSearchFocusMs !== undefined,
  );
  const attribute = await page.locator("html").getAttribute("data-kosh-search-focus-ms");
  if (attribute === null) throw new Error("Command-K focus timing was not recorded");
  const measured = Number(attribute);
  if (!Number.isFinite(measured)) throw new Error("Command-K focus timing was not recorded");
  return measured;
}

async function measureFirstSearchResult(page, index) {
  const query = `baselinequery${index}`;
  await page.evaluate(
    async ({ query: marker, index: sequence }) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      const noteId = `019f547b-6200-7000-8000-${String(sequence + 1).padStart(12, "0")}`;
      const saved = await backend.saveWorkingCopy({
        noteId,
        baseRevisionId: null,
        editGeneration: 1,
        bodyMarkdown: `Deterministic ${marker} passage ${sequence}.`,
        sources: [],
      });
      if (saved.status !== "SAVED") {
        throw new Error(`baseline working copy was not saved: ${saved.status}`);
      }
      const checkpoint = await backend.checkpointWorkingCopy({
        noteId,
        expectedEditGeneration: 1,
      });
      if (checkpoint.status !== "CHECKPOINTED") {
        throw new Error(`baseline note was not checkpointed: ${checkpoint.status}`);
      }
    },
    { query, index },
  );
  const started = performance.now();
  await page.getByRole("combobox", { name: "Search notes" }).fill(query);
  await page.getByRole("option", { name: new RegExp(query, "u") }).waitFor({ state: "visible" });
  return performance.now() - started;
}

async function measureNativeStartup() {
  const nativeOutput = { value: "" };
  const nativeServer = startViteServer(1420, {}, nativeOutput);
  const binary = join(appRoot, "src-tauri/target/debug/kosh");
  try {
    await waitForProcessServer("http://127.0.0.1:1420/", nativeServer, nativeOutput);
    await commandOutput(
      "cargo",
      [
        "build",
        "--locked",
        "--manifest-path",
        "src-tauri/Cargo.toml",
        "--no-default-features",
        "--bin",
        "kosh",
      ],
      appRoot,
    );

    const freshMs = [];
    for (let index = 0; index < nativeSampleCount; index += 1) {
      const profile = await mkdtemp(join(tmpdir(), "kosh-redesign-cold-"));
      try {
        freshMs.push(await runNativeStartup(binary, profile, "absent", index));
      } finally {
        await rm(profile, { force: true, recursive: true });
      }
    }

    const warmProfile = await mkdtemp(join(tmpdir(), "kosh-redesign-warm-"));
    try {
      await runNativeStartup(binary, warmProfile, "absent", "seed");
      const restartMs = [];
      for (let index = 0; index < nativeSampleCount; index += 1) {
        restartMs.push(await runNativeStartup(binary, warmProfile, "present", index));
      }
      return {
        freshHiddenProcessMs: summarize(freshMs),
        restartHiddenProcessMs: summarize(restartMs),
      };
    } finally {
      await rm(warmProfile, { force: true, recursive: true });
    }
  } finally {
    nativeServer.kill("SIGTERM");
    await waitForExit(nativeServer);
  }
}

async function runNativeStartup(binary, dataDirectory, expectation, sample) {
  const receipt = join(dataDirectory, `startup-${expectation}-${sample}.json`);
  const started = performance.now();
  await new Promise((resolveRun, rejectRun) => {
    const child = spawn(binary, [], {
      cwd: appRoot,
      env: {
        ...process.env,
        KOSH_DATA_DIR: dataDirectory,
        KOSH_STARTUP_SMOKE_EXPECT: expectation,
        KOSH_STARTUP_SMOKE_HEAD: buildRevision,
        KOSH_STARTUP_SMOKE_RECEIPT: receipt,
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => {
      output += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      output += chunk.toString();
    });
    const timeout = setTimeout(() => {
      child.kill("SIGTERM");
      rejectRun(new Error(`native startup sample timed out:\n${output}`));
    }, 45_000);
    child.once("error", (error) => {
      clearTimeout(timeout);
      rejectRun(error);
    });
    child.once("exit", (code) => {
      clearTimeout(timeout);
      if (code === 0) resolveRun();
      else rejectRun(new Error(`native startup sample exited ${code}:\n${output}`));
    });
  });
  const parsed = JSON.parse(await readFile(receipt, "utf8"));
  if (parsed.headSha !== buildRevision || parsed.buildHeadSha !== buildRevision) {
    throw new Error("native startup receipt was not bound to the measured build");
  }
  return performance.now() - started;
}

function summarize(samples) {
  const sorted = samples.toSorted((left, right) => left - right);
  return {
    median: round(percentile(sorted, 0.5)),
    p95: round(percentile(sorted, 0.95)),
    min: round(sorted[0]),
    max: round(sorted.at(-1)),
    samples: samples.map(round),
  };
}

function assessBudgets(interactive, nativeStartup, scale, frozenBaseline) {
  const frozen = frozenBaseline.interactive;
  return {
    hiddenNativeStartupP95: budget(
      nativeStartup.freshHiddenProcessMs.p95,
      round(frozenBaseline.nativeStartup.coldProcessMs.p95 * 1.2),
      "hidden exact-head startup regression evidence; visible focus is measured manually",
    ),
    coldShellP95: budget(
      interactive.coldShellMs.p95,
      round(frozen.coldShellMs.p95 * 1.2),
      "within 20% of the frozen shell baseline",
    ),
    editorInitializationP95: budget(
      interactive.editorInitializationMs.p95,
      round(frozen.editorInitializationMs.p95 * 1.3),
      "explicitly reviewed BlockNote ceiling of 30% over the frozen ProseMirror baseline",
    ),
    inputPaintP95: budget(
      interactive.inputPaintMs.p95,
      16.67,
      "ordinary input paints within one 60 Hz frame",
    ),
    searchOverlayP95: budget(interactive.searchNavigationMs.p95, 100, "warm Command-K overlay"),
    firstSearchResultP95: budget(
      interactive.firstSearchResultMs.p95,
      round(frozen.firstSearchResultMs.p95 * 1.2),
      "within 20% of the frozen deterministic result baseline",
    ),
    lexicalScaleP95: budget(
      scale.queryP95Ms,
      scale.interactiveP95BudgetMs,
      "10,000-note production lexical path",
    ),
  };
}

function budget(actual, limit, rationale) {
  return {
    actual,
    limit,
    passed: actual <= limit,
    rationale,
  };
}

function percentile(sorted, quantile) {
  return sorted[Math.max(0, Math.ceil(sorted.length * quantile) - 1)];
}

function round(value) {
  return Math.round(value * 100) / 100;
}

async function waitForServer(url) {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (server.exitCode !== null) {
      throw new Error(`baseline Vite server exited early:\n${serverOutput}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // The bounded retry loop reports the useful server output on failure.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error(`baseline Vite server did not become ready:\n${serverOutput}`);
}

function startViteServer(port, extraEnvironment, output) {
  const child = spawn(
    process.execPath,
    [
      join(appRoot, "node_modules/vite/bin/vite.js"),
      "--host",
      "127.0.0.1",
      "--port",
      String(port),
      "--strictPort",
    ],
    {
      cwd: appRoot,
      env: { ...process.env, NO_COLOR: "1", ...extraEnvironment },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stdout.on("data", (chunk) => {
    output.value += chunk.toString();
  });
  child.stderr.on("data", (chunk) => {
    output.value += chunk.toString();
  });
  return child;
}

async function waitForProcessServer(url, child, output) {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (child.exitCode !== null) {
      throw new Error(`Vite server exited early:\n${output.value}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // The bounded retry loop reports the useful server output on failure.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error(`Vite server did not become ready:\n${output.value}`);
}

async function waitForExit(child) {
  if (child.exitCode !== null) return;
  await new Promise((resolveExit) => child.once("exit", resolveExit));
}

async function commandOutput(command, arguments_, cwd) {
  return new Promise((resolveOutput, rejectOutput) => {
    const child = spawn(command, arguments_, { cwd, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.once("error", rejectOutput);
    child.once("exit", (code) => {
      if (code === 0) resolveOutput(stdout);
      else rejectOutput(new Error(`${command} exited ${code}: ${stderr}`));
    });
  });
}
