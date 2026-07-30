import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { join, resolve, sep } from "node:path";
import { spawn, spawnSync } from "node:child_process";

const appRoot = resolve(import.meta.dirname, "..");
run(process.execPath, [join(appRoot, "scripts/check-release-source.mjs"), resolve(appRoot, "..")]);
const appPath = resolve(
  process.argv[2] ??
    join(appRoot, "src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app"),
);
const executable = join(appPath, "Contents/MacOS/kosh");
const smokeRoot = resolve(appRoot, ".data/release-smoke");
mkdirSync(smokeRoot, { recursive: true });
const ownedRoot = mkdtempSync(join(smokeRoot, "packaged-"));
const home = join(ownedRoot, "home");
const temporary = join(ownedRoot, "tmp");
const headSha = run("git", ["-C", appRoot, "rev-parse", "HEAD"]);
mkdirSync(home);
mkdirSync(temporary);

assertRegularExecutable(executable, "packaged Kosh executable");
assert(/^[0-9a-f]{40}$/u.test(headSha), "release smoke requires an exact Git HEAD");

let active;
try {
  const freshReceiptPath = join(ownedRoot, "fresh-receipt.json");
  const first = await launch("fresh", "absent", freshReceiptPath);
  active = first.child;
  const freshReceipt = await finishLaunch(first, freshReceiptPath);
  active = undefined;
  verifyReceipt(freshReceipt, "absent", true);

  const dataDirectory = realpathSync(freshReceipt.dataDir);
  assert(
    dataDirectory.startsWith(`${realpathSync(home)}${sep}`),
    "packaged smoke escaped its isolated home",
  );
  const mainDatabase = join(dataDirectory, "kosh.sqlite3");
  const mediaDatabase = join(dataDirectory, "media.sqlite3");
  verifyDatabasePair(mainDatabase, mediaDatabase);
  assertEqual(sqlite(mainDatabase, "SELECT count(*) FROM tidbit;"), "1", "fresh canary count");
  assertEqual(
    sqlite(mainDatabase, "SELECT normalized_url FROM source WHERE normalized_url IS NOT NULL;"),
    freshReceipt.canary.sourceUrl,
    "fresh canary source URL",
  );

  const restartReceiptPath = join(ownedRoot, "restart-receipt.json");
  const second = await launch("restart", "present", restartReceiptPath);
  active = second.child;
  const restartReceipt = await finishLaunch(second, restartReceiptPath);
  active = undefined;
  verifyReceipt(restartReceipt, "present", false);
  verifyDatabasePair(mainDatabase, mediaDatabase);
  assertEqual(sqlite(mainDatabase, "SELECT count(*) FROM tidbit;"), "1", "restart canary count");
  assertEqual(restartReceipt.dataDir, freshReceipt.dataDir, "restart data directory");
  assertEqual(restartReceipt.canary, freshReceipt.canary, "restart canary identity");

  const combinedLog = ["fresh", "restart"]
    .map((name) => readFileSync(join(ownedRoot, `${name}.log`), "utf8"))
    .join("\n");
  assert(
    !combinedLog.includes("incompatible") && !combinedLog.includes("panicked"),
    `packaged launch log reports a fatal startup problem:\n${combinedLog}`,
  );
  assert(
    !existsSync(join(dataDirectory, "logs/llama-server.log")),
    "llama-server started without a semantic query",
  );

  console.info(
    `Packaged runtime smoke passed: the release React roots captured a cited canary through IPC, resolved it through exact search on both surfaces, and preserved its identity across restart in ${dataDirectory} without Claude or a semantic model.`,
  );
} finally {
  if (active) {
    active.kill("SIGKILL");
  }
  assert(
    ownedRoot.startsWith(`${smokeRoot}${sep}`) &&
      ownedRoot.split(sep).at(-1)?.startsWith("packaged-"),
    `refusing to clean unexpected smoke root: ${ownedRoot}`,
  );
  rmSync(ownedRoot, { recursive: true });
}

async function launch(label, expectation, receiptPath) {
  const logPath = join(ownedRoot, `${label}.log`);
  const logDescriptor = openSync(logPath, "a");
  const environment = {
    ...process.env,
    HOME: home,
    TMPDIR: temporary,
    PATH: "/usr/bin:/bin",
  };
  for (const name of [
    "KOSH_DATA_DIR",
    "KOSH_LLAMA_SERVER_PATH",
    "KOSH_EMBEDDING_MODEL_PATH",
    "KOSH_STARTUP_SMOKE_RECEIPT",
    "KOSH_STARTUP_SMOKE_HEAD",
    "KOSH_STARTUP_SMOKE_EXPECT",
    "KOSH_CLAUDE_DISABLED",
    "CLAUDE_CONFIG_DIR",
  ]) {
    delete environment[name];
  }
  environment.KOSH_STARTUP_SMOKE_RECEIPT = receiptPath;
  environment.KOSH_STARTUP_SMOKE_HEAD = headSha;
  environment.KOSH_STARTUP_SMOKE_EXPECT = expectation;
  environment.KOSH_CLAUDE_DISABLED = "1";
  const child = spawn(executable, [], {
    env: environment,
    stdio: ["ignore", logDescriptor, logDescriptor],
  });
  child.on("error", () => {});
  return { child, logDescriptor };
}

async function finishLaunch(launchRecord, receiptPath) {
  try {
    await waitForPath(launchRecord.child, receiptPath, 45_000);
    const exited = await Promise.race([
      launchRecord.child.exitCode !== null || launchRecord.child.signalCode !== null
        ? Promise.resolve(true)
        : new Promise((resolveExit) => launchRecord.child.once("close", () => resolveExit(true))),
      delay(5_000).then(() => false),
    ]);
    assert(exited, "packaged Kosh did not exit after writing its smoke receipt");
    assertEqual(launchRecord.child.exitCode, 0, "packaged smoke exit code");
    return JSON.parse(readFileSync(receiptPath, "utf8"));
  } finally {
    closeSync(launchRecord.logDescriptor);
  }
}

async function waitForPath(child, path, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(path)) return;
    assertAlive(child);
    await delay(100);
  }
  throw new Error(`packaged Kosh did not create ${path} within ${timeoutMs}ms`);
}

function verifyReceipt(receipt, expectation, captureExpected) {
  assertEqual(receipt.schemaVersion, 4, "smoke receipt schema");
  assertEqual(receipt.headSha, headSha, "requested smoke head");
  assertEqual(receipt.buildHeadSha, headSha, "packaged executable build head");
  assertEqual(receipt.expectation, expectation, "smoke expectation");
  assertEqual([...receipt.windows].sort(), ["main", "quick-add"], "packaged smoke windows");
  assertEqual(
    receipt.webviews.map(({ surface }) => surface).sort(),
    ["main", "quick-add"],
    "packaged smoke webviews",
  );
  for (const webview of receipt.webviews) {
    assert(webview.rendered && webview.rootChildCount > 0, `${webview.surface} React root`);
    assertEqual(webview.frontendOrigin, "tauri://localhost", `${webview.surface} origin`);
    assertEqual(webview.probeDataDir, receipt.dataDir, `${webview.surface} IPC data directory`);
    assert(
      typeof webview.probeRequestId === "string" && webview.probeRequestId.length > 0,
      `${webview.surface} IPC request ID`,
    );
    assertEqual(
      webview.captureCreated,
      captureExpected && webview.surface === "main",
      `${webview.surface} capture evidence`,
    );
    assertEqual(webview.canary.executionMode, "EXACT", `${webview.surface} search mode`);
    assertEqual(webview.canary.citationState, "CURRENT", `${webview.surface} citation state`);
    assertEqual(webview.canary.resultCount, 1, `${webview.surface} canary result count`);
    assertEqual(webview.canary.passageId, receipt.canary.passageId, `${webview.surface} passage`);
    assertEqual(
      webview.canary.resolvedPassageId,
      receipt.canary.passageId,
      `${webview.surface} resolved passage`,
    );
    assertEqual(
      webview.canary.revisionId,
      receipt.canary.revisionId,
      `${webview.surface} revision`,
    );
    assertEqual(
      webview.canary.sourceUrl,
      receipt.canary.sourceUrl,
      `${webview.surface} source URL`,
    );
  }
  assertEqual(receipt.canaryPreexisting, !captureExpected, "preexisting canary state");
  assertEqual(receipt.canaryCreated, captureExpected, "created canary state");
  assertEqual(
    receipt.canary.sourceUrl,
    "https://example.invalid/kosh-progressive-operability",
    "canary source URL",
  );
}

function verifyDatabasePair(main, media) {
  const expectedMain = readdirSync("src-tauri/src/database/migrations/main").filter((name) =>
    /^V\d+__.+\.sql$/u.test(name),
  ).length;
  const expectedMedia = readdirSync("src-tauri/src/database/migrations/media").filter((name) =>
    /^V\d+__.+\.sql$/u.test(name),
  ).length;
  assertEqual(
    sqlite(main, "SELECT count(*) FROM refinery_schema_history;"),
    String(expectedMain),
    "main migration count",
  );
  assertEqual(
    sqlite(media, "SELECT count(*) FROM refinery_schema_history;"),
    String(expectedMedia),
    "media migration count",
  );
  assertEqual(sqlite(main, "PRAGMA journal_mode;").toLowerCase(), "wal", "main WAL");
  assertEqual(sqlite(media, "PRAGMA journal_mode;").toLowerCase(), "wal", "media WAL");
  assertEqual(sqlite(main, "PRAGMA integrity_check;"), "ok", "main integrity");
  assertEqual(sqlite(media, "PRAGMA integrity_check;"), "ok", "media integrity");
}

function sqlite(database, statement) {
  const result = spawnSync("sqlite3", ["-batch", database, statement], {
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }
  return result.stdout.trim();
}

function run(command, arguments_) {
  const result = spawnSync(command, arguments_, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }
  return result.stdout.trim();
}

function assertRegularExecutable(path, label) {
  const metadata = lstatSync(path);
  assert(metadata.isFile(), `${label} is not a regular file`);
  assert(!metadata.isSymbolicLink(), `${label} must not be a symlink`);
  assert((metadata.mode & 0o111) !== 0, `${label} is not executable`);
}

function assertAlive(child) {
  assert(
    child.exitCode === null && child.signalCode === null,
    `packaged Kosh exited during smoke verification with code ${child.exitCode} and signal ${child.signalCode}`,
  );
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

function assertEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Unexpected ${label}: ${JSON.stringify(actual)}; expected ${JSON.stringify(expected)}`,
    );
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
