import { spawn } from "node:child_process";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { cpus, platform, release, totalmem } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { chromium } from "@playwright/test";

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(appRoot, "..");
const outputPath = resolve(
  appRoot,
  process.argv[2] ?? "fixtures/redesign/blocknote-feasibility-v1.performance.json",
);
const baselinePath = join(appRoot, "fixtures/redesign/baseline-v1.performance.json");
const spikeDist = join(appRoot, ".data/redesign/blocknote-spike-dist");
const baseUrl = "http://127.0.0.1:1422/blocknote-spike.html";
const sampleCount = 10;
const longDocumentBlocks = 500;

const sourceRevision = (await commandOutput("git", ["rev-parse", "HEAD"], repositoryRoot)).trim();
const worktreeStatus = await commandOutput(
  "git",
  ["status", "--porcelain=v1", "--untracked-files=all"],
  repositoryRoot,
);
if (worktreeStatus.trim().length > 0) {
  throw new Error(
    `BlockNote recording requires a clean HEAD; commit or stash these changes first:\n${worktreeStatus}`,
  );
}

const baseline = JSON.parse(await readFile(baselinePath, "utf8"));
const serverOutput = { value: "" };
const server = startViteServer(serverOutput);

try {
  await waitForServer(server, serverOutput);
  const browser = await chromium.launch({ headless: true });
  try {
    const initializationMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      initializationMs.push(await measureInitialization(browser));
    }
    process.stdout.write("Measured BlockNote initialization.\n");

    const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
    const page = await context.newPage();
    await page.goto(baseUrl);
    await waitForBridge(page);
    await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__.appendParagraph());
    const firstInputPaintMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      firstInputPaintMs.push(await measureInputPaint(page, index % 2 === 0 ? "x" : "Backspace"));
    }
    process.stdout.write("Measured first-input paint.\n");

    await page.evaluate(
      (count) => window.__KOSH_BLOCKNOTE_SPIKE__.installLongDocument(count),
      longDocumentBlocks,
    );
    const longDocumentInputPaintMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      longDocumentInputPaintMs.push(
        await measureInputPaint(page, index % 2 === 0 ? "x" : "Backspace"),
      );
    }
    process.stdout.write("Measured 500-block input paint.\n");
    const longDocumentScrollMs = [];
    for (let index = 0; index < sampleCount; index += 1) {
      longDocumentScrollMs.push(await measureScroll(page, index % 2 === 0));
    }
    await context.close();

    const metrics = {
      initializationMs: summarize(initializationMs),
      firstInputPaintMs: summarize(firstInputPaintMs),
      longDocumentInputPaintMs: summarize(longDocumentInputPaintMs),
      longDocumentScrollMs: summarize(longDocumentScrollMs),
    };
    const bundle = await bundleReport(spikeDist);
    const budgets = createBudgets(metrics, baseline);
    const packageEvidence = await blockNotePackageEvidence();
    const report = {
      schemaVersion: 1,
      spike: "restricted-blocknote-browser-feasibility",
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
        samplesPerMetric: sampleCount,
        browserMode: "headless Chromium against the isolated browser-only BlockNote harness",
        initialization: "new browser context navigation through installed BlockNote capability",
        inputPaint: "beforeinput through the next animation frame",
        longDocumentBlocks,
        scroll: "programmatic top/bottom scroll through two animation frames",
        bundle: "minified standalone spike assets with gzip level 9",
      },
      metrics,
      budgets,
      bundle: {
        ...bundle,
        currentProductionEntryImpactBytes: 0,
        isolation:
          "blocknote-spike.html is excluded from the production Vite inputs; repository bundle isolation remains authoritative",
      },
      packageEvidence,
      boundedRemediation: [
        "Keep BlockNote out of the cold shell chunk and lazy-load the note editor boundary.",
        "Reuse Kosh system fonts instead of shipping BlockNote's optional Inter font assets.",
        "Retain the restricted schema and custom UI surface so unsupported blocks and XL packages never enter the bundle.",
      ],
    };
    await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
    process.stdout.write(`Wrote ${outputPath}\n`);
    if (!budgets.firstInputPaint.passed || !budgets.longDocumentInputPaint.passed) {
      throw new Error("BlockNote input latency exceeded the one-frame feasibility budget");
    }
  } finally {
    await browser.close();
  }
} finally {
  server.kill("SIGTERM");
  await waitForExit(server);
}

async function measureInitialization(browser) {
  const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
  const page = await context.newPage();
  const started = performance.now();
  await page.goto(baseUrl);
  await waitForBridge(page);
  const duration = performance.now() - started;
  await context.close();
  return duration;
}

async function measureInputPaint(page, key) {
  await page.evaluate(() => {
    const editor = document.querySelector(".bn-editor");
    if (!editor) throw new Error("BlockNote editor is unavailable");
    let started;
    let beforeInput;
    let input;
    window.__KOSH_BLOCKNOTE_INPUT_SAMPLE__ = new Promise((resolveSample) => {
      const finish = (result) => {
        clearTimeout(timeout);
        editor.removeEventListener("beforeinput", beforeInput);
        editor.removeEventListener("input", input);
        resolveSample(result);
      };
      beforeInput = () => {
        started = performance.now();
      };
      input = () => {
        requestAnimationFrame(() =>
          finish(
            started === undefined
              ? { error: "input arrived without beforeinput" }
              : { duration: performance.now() - started },
          ),
        );
      };
      const timeout = setTimeout(
        () => finish({ error: "timed out waiting for BlockNote input paint" }),
        5_000,
      );
      editor.addEventListener("beforeinput", beforeInput);
      editor.addEventListener("input", input);
    });
  });
  if (key === "Backspace") await page.keyboard.press(key);
  else await page.keyboard.insertText(key);
  const sample = await page.evaluate(async () => {
    const result = await window.__KOSH_BLOCKNOTE_INPUT_SAMPLE__;
    delete window.__KOSH_BLOCKNOTE_INPUT_SAMPLE__;
    return result;
  });
  if (sample.error) throw new Error(sample.error);
  return sample.duration;
}

async function measureScroll(page, toBottom) {
  return page.evaluate(
    ({ bottom }) =>
      new Promise((resolveScroll) => {
        const started = performance.now();
        window.scrollTo({ top: bottom ? document.documentElement.scrollHeight : 0 });
        requestAnimationFrame(() => {
          requestAnimationFrame(() => resolveScroll(performance.now() - started));
        });
      }),
    { bottom: toBottom },
  );
}

function createBudgets(metrics, baseline) {
  const baselineInitialization = baseline.interactive.editorInitializationMs.p95;
  const baselineInput = baseline.interactive.inputPaintMs.p95;
  return {
    initialization: budget(
      metrics.initializationMs.p95,
      round(baselineInitialization * 1.2),
      "within 20% of the frozen pre-redesign editor initialization p95",
    ),
    firstInputPaint: budget(
      metrics.firstInputPaintMs.p95,
      16.67,
      "within one 60 Hz animation frame",
    ),
    baselineInputComparison: budget(
      metrics.firstInputPaintMs.p95,
      round(baselineInput * 1.2),
      "within 20% of the frozen pre-redesign input-paint p95",
    ),
    longDocumentInputPaint: budget(
      metrics.longDocumentInputPaintMs.p95,
      16.67,
      "500-block input remains within one 60 Hz animation frame",
    ),
    longDocumentScroll: budget(
      metrics.longDocumentScrollMs.p95,
      34,
      "two animation frames for a 500-block top/bottom scroll",
    ),
  };
}

function budget(actual, limit, rationale) {
  return { actual, limit, passed: actual <= limit, rationale };
}

async function blockNotePackageEvidence() {
  const packages = await Promise.all(
    ["@blocknote/core", "@blocknote/react", "@blocknote/mantine"].map(async (name) => {
      const packageJson = JSON.parse(
        await readFile(join(appRoot, "node_modules", name, "package.json"), "utf8"),
      );
      return { name, version: packageJson.version, license: packageJson.license };
    }),
  );
  return {
    included: packages,
    excludedPackagePrefixes: ["@blocknote/xl-"],
  };
}

async function bundleReport(root) {
  const files = await listFiles(root);
  const entries = await Promise.all(
    files.map(async (path) => {
      const bytes = await readFile(path);
      return {
        path: path.slice(root.length + 1),
        extension: extname(path),
        rawBytes: bytes.length,
        gzipBytes: gzipSync(bytes, { level: 9 }).length,
      };
    }),
  );
  const code = entries.filter((entry) => entry.extension === ".js" || entry.extension === ".css");
  return {
    files: entries,
    codeRawBytes: code.reduce((total, entry) => total + entry.rawBytes, 0),
    codeGzipBytes: code.reduce((total, entry) => total + entry.gzipBytes, 0),
    totalRawBytes: entries.reduce((total, entry) => total + entry.rawBytes, 0),
    totalGzipBytes: entries.reduce((total, entry) => total + entry.gzipBytes, 0),
  };
}

async function listFiles(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map((entry) => {
      const path = join(root, entry.name);
      return entry.isDirectory() ? listFiles(path) : [path];
    }),
  );
  return nested.flat().toSorted();
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

function percentile(sorted, quantile) {
  return sorted[Math.max(0, Math.ceil(sorted.length * quantile) - 1)];
}

function round(value) {
  return Math.round(value * 100) / 100;
}

function startViteServer(output) {
  const child = spawn(
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
  child.stdout.on("data", (chunk) => {
    output.value += chunk.toString();
  });
  child.stderr.on("data", (chunk) => {
    output.value += chunk.toString();
  });
  return child;
}

async function waitForServer(child, output) {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (child.exitCode !== null) throw new Error(`Vite exited early:\n${output.value}`);
    try {
      const response = await fetch(baseUrl);
      if (response.ok) return;
    } catch {
      // The bounded retry loop reports server output after exhaustion.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error(`Vite did not become ready:\n${output.value}`);
}

async function waitForBridge(page) {
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_SPIKE__?.capability === "blocknote");
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
