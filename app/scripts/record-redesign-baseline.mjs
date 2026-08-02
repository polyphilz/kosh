import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { cpus, platform, release, tmpdir, totalmem } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(appRoot, "..");
const outputPath = resolve(
  appRoot,
  process.argv[2] ?? "fixtures/redesign/baseline-v1.performance.json",
);
const scaleReportPath = join(appRoot, ".data/relevance/reports/lexical-scale-v1.performance.json");
const sampleCount = 10;
const nativeSampleCount = 5;
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
      coldShellMs.push(await measureColdSearch(browser));
      editorInitializationMs.push(await measureEditorInitialization(browser));
    }

    const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
    const page = await context.newPage();
    await page.goto(`${baseUrl}/#/add`);
    const editor = page.getByRole("textbox", { name: "Tidbit" });
    await editor.waitFor({ state: "visible" });
    await editor.focus();
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
      await searchPage.goto(`${baseUrl}/#/add`);
      searchNavigationMs.push(await measureSearchNavigation(searchPage));
      firstSearchResultMs.push(await measureFirstSearchResult(searchPage, index));
      await searchContext.close();
    }
    const nativeStartup = await measureNativeStartup();

    const report = {
      schemaVersion: 1,
      baseline: "pre-redesign-current-ui",
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
        coldShell: "new browser context to focused current Search input",
        editorInitialization: "new browser context to visible current ProseMirror editor",
        inputPaint: "beforeinput event through the next animation frame",
        searchNavigation: "Playwright wall time from current Add route to focused Search route",
        firstSearchResult: "Playwright wall time from fill to first deterministic result option",
        lexicalScale: "existing release-mode 10,000-note / 200-query benchmark",
        nativeStartup:
          "process spawn through the complete exact-head Tauri startup-smoke receipt; cold uses a fresh profile and warm reopens one preserved profile",
      },
      interactive: {
        coldShellMs: summarize(coldShellMs),
        editorInitializationMs: summarize(editorInitializationMs),
        inputPaintMs: summarize(inputPaintMs),
        searchNavigationMs: summarize(searchNavigationMs),
        firstSearchResultMs: summarize(firstSearchResultMs),
      },
      nativeStartup,
      lexicalScale: scale,
    };
    await mkdir(dirname(outputPath), { recursive: true });
    await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
    process.stdout.write(`Wrote ${outputPath}\n`);
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

async function measureColdSearch(browser) {
  const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
  const page = await context.newPage();
  const started = performance.now();
  await page.goto(`${baseUrl}/#/`);
  const search = page.getByRole("searchbox", { name: "Search tidbits" });
  await search.waitFor({ state: "visible" });
  await search.focus();
  const duration = performance.now() - started;
  await context.close();
  return duration;
}

async function measureEditorInitialization(browser) {
  const context = await browser.newContext({ locale: "en-US", timezoneId: "UTC" });
  const page = await context.newPage();
  const started = performance.now();
  await page.goto(`${baseUrl}/#/add`);
  await page.getByRole("textbox", { name: "Tidbit" }).waitFor({ state: "visible" });
  const duration = performance.now() - started;
  await context.close();
  return duration;
}

async function measureNextInputPaint(page) {
  return page.evaluate(
    () =>
      new Promise((resolvePaint) => {
        const editor = document.querySelector("[role='textbox'][aria-label='Tidbit']");
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
  const started = performance.now();
  await page.getByRole("link", { name: "Search", exact: true }).click();
  const search = page.getByRole("searchbox", { name: "Search tidbits" });
  await search.waitFor({ state: "visible" });
  await search.focus();
  return performance.now() - started;
}

async function measureFirstSearchResult(page, index) {
  const query = `baselinequery${index}`;
  await page.evaluate(
    async ({ query: marker, index: sequence }) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      await backend.createTidbit({
        title: null,
        bodyMarkdown: `Deterministic ${marker} passage ${sequence}.`,
        sources: [],
      });
    },
    { query, index },
  );
  const started = performance.now();
  await page.getByRole("searchbox", { name: "Search tidbits" }).fill(query);
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

    const coldMs = [];
    for (let index = 0; index < nativeSampleCount; index += 1) {
      const profile = await mkdtemp(join(tmpdir(), "kosh-redesign-cold-"));
      try {
        coldMs.push(await runNativeStartup(binary, profile, "absent", index));
      } finally {
        await rm(profile, { force: true, recursive: true });
      }
    }

    const warmProfile = await mkdtemp(join(tmpdir(), "kosh-redesign-warm-"));
    try {
      await runNativeStartup(binary, warmProfile, "absent", "seed");
      const warmMs = [];
      for (let index = 0; index < nativeSampleCount; index += 1) {
        warmMs.push(await runNativeStartup(binary, warmProfile, "present", index));
      }
      return {
        coldProcessMs: summarize(coldMs),
        warmProcessMs: summarize(warmMs),
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
