import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
} from "node:fs";
import { join, resolve, sep } from "node:path";
import { spawn, spawnSync } from "node:child_process";

const appRoot = resolve(import.meta.dirname, "..");
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
mkdirSync(home);
mkdirSync(temporary);

assertRegularExecutable(executable, "packaged Kosh executable");

let active;
try {
  const first = await launch("fresh");
  active = first.child;
  const mainDatabase = await waitForDatabase(first.child, "kosh.sqlite3", 30_000);
  const dataDirectory = resolve(mainDatabase, "..");
  const mediaDatabase = join(dataDirectory, "media.sqlite3");
  await waitForPath(first.child, mediaDatabase, 10_000);
  verifyDatabasePair(mainDatabase, mediaDatabase);
  await stop(first.child, first.logDescriptor);
  active = undefined;

  const firstMainCount = sqlite(mainDatabase, "SELECT count(*) FROM tidbit;");
  const second = await launch("restart");
  active = second.child;
  await waitForAlive(second.child, 2_000);
  verifyDatabasePair(mainDatabase, mediaDatabase);
  assertEqual(
    sqlite(mainDatabase, "SELECT count(*) FROM tidbit;"),
    firstMainCount,
    "restart tidbit count",
  );
  await stop(second.child, second.logDescriptor);
  active = undefined;

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
    `Packaged runtime smoke passed: fresh launch and restart used ${dataDirectory}, current migrations and WAL were healthy, capture storage initialized without Claude or a semantic model.`,
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

async function launch(label) {
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
    "CLAUDE_CONFIG_DIR",
  ]) {
    delete environment[name];
  }
  const child = spawn(executable, [], {
    env: environment,
    stdio: ["ignore", logDescriptor, logDescriptor],
  });
  child.on("error", () => {});
  return { child, logDescriptor };
}

async function waitForDatabase(child, filename, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    assertAlive(child);
    const candidate = findFile(home, filename);
    if (candidate) return candidate;
    await delay(100);
  }
  throw new Error(`packaged Kosh did not create ${filename} within ${timeoutMs}ms`);
}

async function waitForPath(child, path, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    assertAlive(child);
    if (existsSync(path)) return;
    await delay(100);
  }
  throw new Error(`packaged Kosh did not create ${path} within ${timeoutMs}ms`);
}

async function waitForAlive(child, durationMs) {
  const deadline = Date.now() + durationMs;
  while (Date.now() < deadline) {
    assertAlive(child);
    await delay(100);
  }
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

async function stop(child, logDescriptor) {
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
  }
  const exited = await Promise.race([
    new Promise((resolveExit) => child.once("close", () => resolveExit(true))),
    delay(5_000).then(() => false),
  ]);
  if (!exited) {
    child.kill("SIGKILL");
    await new Promise((resolveExit) => child.once("close", resolveExit));
  }
  closeSync(logDescriptor);
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

function findFile(directory, filename, depth = 0) {
  if (depth > 8 || !existsSync(directory)) return undefined;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) continue;
    if (entry.isFile() && entry.name === filename) return path;
    if (entry.isDirectory()) {
      const match = findFile(path, filename, depth + 1);
      if (match) return match;
    }
  }
  return undefined;
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
  if (actual !== expected) {
    throw new Error(`Unexpected ${label}: ${actual}; expected ${expected}`);
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
