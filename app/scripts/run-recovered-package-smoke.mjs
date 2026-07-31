import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";

const [appArgument, homeArgument, reportArgument] = process.argv.slice(2);
assert(
  appArgument && homeArgument && reportArgument,
  "expected Kosh.app, isolated home, and report",
);
const appRoot = resolve(import.meta.dirname, "..");
const app = resolve(appArgument);
const home = realpathSync(resolve(homeArgument));
const reportPath = resolve(reportArgument);
const executable = join(app, "Contents/MacOS/kosh");
const dataDirectory = join(home, "Library/Application Support/com.rohan.kosh");
const temporary = join(home, "tmp");
const receiptPath = join(home, "packaged-recovery-startup-receipt.json");
const logPath = join(home, "packaged-recovery-startup.log");
const headSha = run("git", ["-C", appRoot, "rev-parse", "HEAD"]);

assertRegularExecutable(executable, "packaged Kosh executable");
assert(
  realpathSync(dataDirectory).startsWith(`${home}/`),
  "restored profile escaped isolated home",
);
assert(/^[0-9a-f]{40}$/u.test(headSha), "packaged recovery smoke needs an exact Git head");
mkdirSync(temporary, { recursive: true, mode: 0o700 });
const descriptor = openSync(logPath, "a", 0o600);
const environment = { ...process.env, HOME: home, TMPDIR: temporary, PATH: "/usr/bin:/bin" };
for (const name of Object.keys(environment)) {
  if (
    name.startsWith("KOSH_LITESTREAM_R2_") ||
    name.startsWith("KOSH_R2_CANARY_") ||
    [
      "KOSH_DATA_DIR",
      "KOSH_LLAMA_SERVER_PATH",
      "KOSH_LITESTREAM_PATH",
      "KOSH_EMBEDDING_MODEL_PATH",
      "KOSH_STARTUP_SMOKE_RECEIPT",
      "KOSH_STARTUP_SMOKE_HEAD",
      "KOSH_STARTUP_SMOKE_EXPECT",
      "CLAUDE_CONFIG_DIR",
    ].includes(name)
  ) {
    delete environment[name];
  }
}
environment.KOSH_STARTUP_SMOKE_RECEIPT = receiptPath;
environment.KOSH_STARTUP_SMOKE_HEAD = headSha;
environment.KOSH_STARTUP_SMOKE_EXPECT = "present";
environment.KOSH_CLAUDE_DISABLED = "1";

const child = spawn(executable, [], {
  env: environment,
  stdio: ["ignore", descriptor, descriptor],
});
child.on("error", () => {});
try {
  await waitForReceipt(child, receiptPath, 45_000);
  const exited = await Promise.race([
    child.exitCode !== null || child.signalCode !== null
      ? Promise.resolve(true)
      : new Promise((resolveExit) => child.once("close", () => resolveExit(true))),
    delay(5_000).then(() => false),
  ]);
  assert(exited, "restored packaged Kosh did not exit after its hidden startup smoke");
  assertEqual(child.exitCode, 0, "restored packaged Kosh exit code");
} finally {
  closeSync(descriptor);
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
}

const receipt = JSON.parse(readFileSync(receiptPath, "utf8"));
assertEqual(receipt.schemaVersion, 4, "startup receipt schema");
assertEqual(receipt.headSha, headSha, "startup requested head");
assertEqual(receipt.buildHeadSha, headSha, "packaged build head");
assertEqual(receipt.expectation, "present", "restored canary expectation");
assertEqual(receipt.canaryPreexisting, true, "restored canary preexistence");
assertEqual(receipt.canaryCreated, false, "restored canary creation");
assertEqual(
  receipt.canary.sourceUrl,
  "https://example.invalid/kosh-progressive-operability",
  "source",
);
assertEqual(
  realpathSync(receipt.dataDir),
  realpathSync(dataDirectory),
  "restored runtime data root",
);

const main = join(dataDirectory, "kosh.sqlite3");
const media = join(dataDirectory, "media.sqlite3");
const evidence = {
  activeTidbits: numberSql(main, "SELECT count(*) FROM tidbit WHERE deleted_at IS NULL"),
  revisions: numberSql(main, "SELECT count(*) FROM tidbit_revision"),
  attachments: numberSql(main, "SELECT count(*) FROM attachment"),
  mediaBlobs: numberSql(media, "SELECT count(*) FROM media_blob"),
  searchDocuments: numberSql(main, "SELECT count(*) FROM passage_search_document"),
  researchCitations: numberSql(
    main,
    "SELECT coalesce(sum(json_array_length(final_answer_json, '$.citations')), 0) FROM research_run WHERE final_answer_json IS NOT NULL",
  ),
};
assert(evidence.activeTidbits >= 2, "restored package lost tidbits");
assert(evidence.revisions >= 3, "restored package lost immutable revisions");
assert(evidence.attachments >= 1 && evidence.mediaBlobs >= 1, "restored package lost media");
assert(evidence.searchDocuments >= 2, "restored package lost rebuilt lexical search");
assert(evidence.researchCitations >= 1, "restored package lost research citations");
writeFileSync(
  reportPath,
  `${JSON.stringify(
    {
      schemaVersion: 1,
      result: "PASSED",
      headSha,
      appPath: app,
      dataDirectory,
      startupReceipt: receiptPath,
      evidence,
    },
    null,
    2,
  )}\n`,
  { mode: 0o600 },
);
console.info("Hidden packaged recovery startup passed with tidbits, media, search, and citations.");

async function waitForReceipt(process, path, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(path)) return;
    assert(
      process.exitCode === null && process.signalCode === null,
      "restored packaged Kosh exited before writing its receipt",
    );
    await delay(100);
  }
  throw new Error(`restored packaged Kosh did not create ${path} within ${timeoutMs}ms`);
}

function numberSql(database, statement) {
  const value = Number(run("sqlite3", ["-batch", "-noheader", database, statement]));
  assert(Number.isSafeInteger(value) && value >= 0, `invalid SQLite count: ${statement}`);
  return value;
}

function run(command, arguments_) {
  const result = spawnSync(command, arguments_, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    throw new Error(`${command} failed with exit ${result.status}`);
  }
  return result.stdout.trim();
}

function assertRegularExecutable(path, label) {
  const metadata = lstatSync(path);
  assert(metadata.isFile() && !metadata.isSymbolicLink(), `${label} is not a regular file`);
  assert((metadata.mode & 0o111) !== 0, `${label} is not executable`);
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
