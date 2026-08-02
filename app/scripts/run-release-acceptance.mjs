import { createHash } from "node:crypto";
import {
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve, sep } from "node:path";
import { spawn, spawnSync } from "node:child_process";

import { selectMigrationSnapshot } from "./select-migration-snapshot.mjs";

const command = process.argv[2] ?? "help";
const arguments_ = process.argv.slice(3);
const appRoot = resolve(import.meta.dirname, "..");
const acceptanceRoot = resolve(appRoot, ".data/release-acceptance");
const defaultApp = resolve(
  appRoot,
  "src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app",
);
const fixtureRoot = resolve(appRoot, "src-tauri/src/database/fixtures/v16-profile");

mkdirSync(acceptanceRoot, { recursive: true });

switch (command) {
  case "prepare-clean":
    prepareClean(arguments_);
    break;
  case "launch":
    launch(arguments_);
    break;
  case "launch-hidden":
    launchHidden(arguments_);
    break;
  case "check-core":
    checkCore(arguments_);
    break;
  case "check-journeys":
    checkJourneys(arguments_);
    break;
  case "checkpoint-restart":
    checkpointRestart(arguments_);
    break;
  case "check-restart":
    checkRestart(arguments_);
    break;
  case "prepare-upgrade":
    prepareUpgrade(arguments_);
    break;
  case "check-upgrade":
    checkUpgrade(arguments_);
    break;
  case "prove-upgrade-recovery":
    proveUpgradeRecovery(arguments_);
    break;
  case "help":
    printUsage();
    break;
  default:
    throw new Error(`unknown release-acceptance command: ${command}`);
}

function prepareClean(values) {
  assert(values.length === 1, "prepare-clean accepts exactly one profile name");
  const profile = newProfile(values[0]);
  initializeProfile(profile);
  console.info(`Prepared empty packaged-app profile: ${profile.root}`);
}

function launch(values) {
  assert(
    values.length >= 1 && values.length <= 2,
    "launch accepts a profile name and optional Kosh.app path",
  );
  const profile = existingProfile(values[0]);
  const app = packagedApp(values[1]);
  assertStopped(profile);

  const log = join(profile.root, "packaged-app.log");
  const environment = packagedEnvironment(profile);
  const descriptor = openSync(log, "a");
  const child = spawn(app.executable, [], {
    detached: true,
    env: environment,
    stdio: ["ignore", descriptor, descriptor],
  });
  assert(Number.isInteger(child.pid), "packaged Kosh did not start");
  child.unref();
  closeSync(descriptor);
  writeJsonReplacing(join(profile.root, "launch.json"), {
    schemaVersion: 1,
    launchedAt: new Date().toISOString(),
    pid: child.pid,
    appPath: app.path,
    executable: app.executable,
    home: profile.home,
    dataDirectory: profile.data,
    guiPath: environment.PATH,
  });
  console.info(
    `Launched packaged Kosh (pid ${child.pid}) against ${profile.data}. Quit with Cmd+Q before running a check.`,
  );
}

function launchHidden(values) {
  assert(
    values.length >= 2 && values.length <= 3,
    "launch-hidden accepts a profile name, absent|present, and optional Kosh.app path",
  );
  const profile = existingProfile(values[0]);
  const expectation = values[1];
  assert(
    expectation === "absent" || expectation === "present",
    "launch-hidden expectation must be absent or present",
  );
  const app = packagedApp(values[2]);
  assertStopped(profile);

  const headResult = spawnSync("git", ["-C", appRoot, "rev-parse", "HEAD"], {
    encoding: "utf8",
  });
  requireSuccess(headResult, "read exact release head");
  const headSha = headResult.stdout.trim();
  assert(/^[0-9a-f]{40}$/u.test(headSha), "release head is not an exact Git commit");
  const statusResult = spawnSync(
    "git",
    ["-C", appRoot, "status", "--porcelain", "--untracked-files=normal"],
    { encoding: "utf8" },
  );
  requireSuccess(statusResult, "inspect release source tree");
  assert(statusResult.stdout === "", "launch-hidden requires a clean source tree");

  const receiptPath = join(
    profile.root,
    `hidden-smoke-${headSha.slice(0, 12)}-${expectation}.json`,
  );
  assert(!existsSync(receiptPath), `hidden receipt already exists: ${receiptPath}`);
  const environment = packagedEnvironment(profile);
  environment.KOSH_STARTUP_SMOKE_RECEIPT = receiptPath;
  environment.KOSH_STARTUP_SMOKE_HEAD = headSha;
  environment.KOSH_STARTUP_SMOKE_EXPECT = expectation;

  const launchedAt = new Date().toISOString();
  const result = spawnSync(app.executable, [], {
    encoding: "utf8",
    env: environment,
    timeout: 45_000,
  });
  requireSuccess(result, "hidden packaged Kosh smoke");
  assert(existsSync(receiptPath), "hidden packaged Kosh wrote no smoke receipt");
  const receipt = readJson(receiptPath);
  verifyHiddenReceipt(receipt, profile, headSha, expectation);

  writeJsonReplacing(join(profile.root, "launch.json"), {
    schemaVersion: 1,
    launchedAt,
    pid: result.pid,
    appPath: app.path,
    executable: app.executable,
    home: profile.home,
    dataDirectory: profile.data,
    guiPath: environment.PATH,
    executionMode: "hidden-smoke",
    receipt: receiptPath,
  });
  console.info(`Hidden packaged acceptance passed for ${basename(profile.root)} at ${headSha}.`);
}

function verifyHiddenReceipt(receipt, profile, headSha, expectation) {
  assertEqual(receipt.schemaVersion, 4, "hidden smoke receipt schema");
  assertEqual(receipt.headSha, headSha, "hidden smoke requested head");
  assertEqual(receipt.buildHeadSha, headSha, "hidden smoke packaged build head");
  assertEqual(receipt.expectation, expectation, "hidden smoke expectation");
  assertEqual(receipt.dataDir, profile.data, "hidden smoke data directory");
  assertEqual([...receipt.windows].sort(), ["main", "quick-add"], "hidden smoke windows");
  assertEqual(
    receipt.webviews.map(({ surface }) => surface).sort(),
    ["main", "quick-add"],
    "hidden smoke webviews",
  );
  for (const webview of receipt.webviews) {
    assert(webview.rendered && webview.rootChildCount > 0, `${webview.surface} React root`);
    assertEqual(webview.frontendOrigin, "tauri://localhost", `${webview.surface} origin`);
    assertEqual(webview.probeDataDir, profile.data, `${webview.surface} IPC data directory`);
    assertEqual(webview.canary.executionMode, "EXACT", `${webview.surface} search mode`);
    assertEqual(webview.canary.citationState, "CURRENT", `${webview.surface} citation state`);
  }
}

function packagedEnvironment(profile) {
  const environment = {
    ...process.env,
    HOME: profile.home,
    TMPDIR: profile.temporary,
    PATH: "/usr/bin:/bin",
  };
  for (const name of [
    "KOSH_DATA_DIR",
    "KOSH_LLAMA_SERVER_PATH",
    "KOSH_LITESTREAM_PATH",
    "KOSH_LITESTREAM_R2_ACCOUNT_ID",
    "KOSH_LITESTREAM_R2_JURISDICTION",
    "KOSH_LITESTREAM_R2_BUCKET",
    "KOSH_LITESTREAM_R2_PREFIX",
    "KOSH_LITESTREAM_R2_ACCESS_KEY_ID",
    "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY",
    "KOSH_EMBEDDING_MODEL_PATH",
    "KOSH_STARTUP_SMOKE_RECEIPT",
    "KOSH_STARTUP_SMOKE_HEAD",
    "KOSH_STARTUP_SMOKE_EXPECT",
  ]) {
    delete environment[name];
  }
  return environment;
}

function checkCore(values) {
  assert(values.length === 1, "check-core accepts exactly one profile name");
  const profile = existingProfile(values[0]);
  assertStopped(profile);
  verifyDatabasePair(profile);
  assert(
    numberSql(profile.main, "SELECT count(*) FROM tidbit WHERE deleted_at IS NULL") >= 1,
    "create at least one active tidbit before checking core acceptance",
  );
  assert(
    numberSql(profile.main, "SELECT count(*) FROM passage_search_document") >= 1,
    "lexical search projection is empty",
  );
  console.info(
    `Core packaged acceptance passed: ${logicalEvidence(profile).activeTidbits} active notes, healthy WAL databases, current migrations, and lexical search available without a semantic model.`,
  );
}

function checkJourneys(values) {
  assert(values.length === 1, "check-journeys accepts exactly one profile name");
  const profile = existingProfile(values[0]);
  assertStopped(profile);
  verifyDatabasePair(profile);

  const requirements = [
    ["a URL-bearing source", "SELECT count(*) FROM source WHERE normalized_url IS NOT NULL"],
    [
      "a code-bearing tidbit",
      "SELECT count(*) FROM tidbit_revision WHERE instr(body_markdown, '```') > 0",
    ],
    [
      "a math-bearing tidbit",
      "SELECT count(*) FROM tidbit_revision WHERE instr(body_markdown, '$') > 0",
    ],
    ["an image attachment", "SELECT count(*) FROM attachment_image"],
    [
      "a completed image OCR extraction",
      "SELECT count(*) FROM attachment_extraction WHERE extractor = 'ocr' AND status = 'READY'",
    ],
    ["a PDF attachment", "SELECT count(*) FROM attachment_pdf"],
    [
      "a completed PDF extraction",
      "SELECT count(*) FROM attachment_extraction WHERE extractor = 'pdf-text' AND status = 'READY'",
    ],
    [
      "searchable extracted attachment text",
      "SELECT count(*) FROM passage_search_document WHERE length(extracted_text) > 0",
    ],
    ["a semantic passage embedding", "SELECT count(*) FROM passage_embedding"],
  ];
  for (const [label, statement] of requirements) {
    assert(
      numberSql(profile.main, statement) >= 1,
      `packaged journey evidence is missing ${label}`,
    );
  }
  console.info(
    "Packaged journey acceptance passed: rich capture, source citations, image OCR, PDF extraction/search, and semantic indexing are durable.",
  );
}

function checkpointRestart(values) {
  assert(values.length === 1, "checkpoint-restart accepts exactly one profile name");
  const profile = existingProfile(values[0]);
  assertStopped(profile);
  verifyDatabasePair(profile);
  writeJson(join(profile.root, "restart-checkpoint.json"), {
    schemaVersion: 1,
    recordedAt: new Date().toISOString(),
    evidence: logicalEvidence(profile),
  });
  console.info(
    "Recorded durable evidence. Relaunch the same profile, inspect search/citations, quit with Cmd+Q, then run check-restart.",
  );
}

function checkRestart(values) {
  assert(values.length === 1, "check-restart accepts exactly one profile name");
  const profile = existingProfile(values[0]);
  assertStopped(profile);
  verifyDatabasePair(profile);
  const checkpoint = readJson(join(profile.root, "restart-checkpoint.json"));
  assertEqual(logicalEvidence(profile), checkpoint.evidence, "logical restart evidence");
  console.info(
    "Packaged restart passed: authored, attachment, search, semantic, and legacy row counts survived unchanged.",
  );
}

function prepareUpgrade(values) {
  assert(values.length === 1, "prepare-upgrade accepts exactly one profile name");
  const profile = newProfile(values[0]);
  initializeProfile(profile);
  mkdirSync(profile.data, { recursive: true });
  materializeSql(profile.main, readFileSync(join(fixtureRoot, "main-v16.sql"), "utf8"));
  materializeSql(profile.media, readFileSync(join(fixtureRoot, "media-v2.sql"), "utf8"));
  console.info(
    `Prepared previous-release V16/V2 profile: ${profile.data}. Launch it, inspect the fixture, and quit with Cmd+Q before check-upgrade.`,
  );
}

function checkUpgrade(values) {
  assert(values.length === 1, "check-upgrade accepts exactly one profile name");
  const profile = existingProfile(values[0]);
  assertStopped(profile);
  verifyDatabasePair(profile);
  const fixture = readJson(join(fixtureRoot, "manifest.json"));
  assertEqual(
    textSql(
      profile.main,
      `SELECT current_revision_id FROM tidbit WHERE id = '${fixture.tidbitId}'`,
    ),
    fixture.revisionId,
    "upgraded fixture revision",
  );
  assertEqual(
    textSql(
      profile.main,
      `SELECT normalized_url
       FROM source
       JOIN tidbit_revision_source ON tidbit_revision_source.source_id = source.id
       WHERE tidbit_revision_source.tidbit_revision_id = '${fixture.revisionId}'`,
    ),
    fixture.sourceUrl,
    "upgraded fixture source",
  );
  assert(
    numberSql(
      profile.main,
      `SELECT count(*) FROM passage_search_document WHERE body LIKE '%amber%'`,
    ) >= 1,
    "upgraded fixture is absent from lexical search",
  );
  const snapshot = migrationSnapshot(
    profile,
    fixture.mainMigrationHead,
    fixture.mediaMigrationHead,
  );
  verifySnapshot(snapshot);
  assertEqual(
    textSql(snapshot.main, "SELECT max(version) FROM refinery_schema_history"),
    String(fixture.mainMigrationHead),
    "snapshot main migration head",
  );
  assertEqual(
    textSql(snapshot.media, "SELECT max(version) FROM refinery_schema_history"),
    String(fixture.mediaMigrationHead),
    "snapshot media migration head",
  );
  console.info(
    `Packaged upgrade passed: V${fixture.mainMigrationHead}/V${fixture.mediaMigrationHead} content, exact search, and citation source survived; ${basename(snapshot.directory)} is a verified pre-migration recovery point.`,
  );
}

function proveUpgradeRecovery(values) {
  assert(values.length === 2, "prove-upgrade-recovery accepts source and new target profile names");
  const source = existingProfile(values[0]);
  assertStopped(source);
  const fixture = readJson(join(fixtureRoot, "manifest.json"));
  const snapshot = migrationSnapshot(source, fixture.mainMigrationHead, fixture.mediaMigrationHead);
  verifySnapshot(snapshot);
  const target = newProfile(values[1]);
  initializeProfile(target);
  mkdirSync(target.data, { recursive: true });
  copyFileSync(snapshot.main, target.main);
  copyFileSync(snapshot.media, target.media);
  writeJson(join(target.root, "recovery-evidence.json"), {
    schemaVersion: 1,
    recoveredAt: new Date().toISOString(),
    sourceProfile: basename(source.root),
    sourceSnapshot: snapshot.directory,
    mainSha256: sha256File(target.main),
    mediaSha256: sha256File(target.media),
  });
  console.info(
    `Recovered the verified pre-migration pair into ${target.data}. Launch ${basename(target.root)}, quit, then run check-upgrade on it to prove the replacement profile upgrades and reopens.`,
  );
}

function initializeProfile(profile) {
  mkdirSync(profile.root);
  mkdirSync(profile.home);
  mkdirSync(profile.temporary);
}

function verifyDatabasePair(profile) {
  assertRegularFile(profile.main, "main database");
  assertRegularFile(profile.media, "media database");
  assertEqual(textSql(profile.main, "PRAGMA integrity_check"), "ok", "main integrity");
  assertEqual(textSql(profile.media, "PRAGMA integrity_check"), "ok", "media integrity");
  assertEqual(textSql(profile.main, "PRAGMA journal_mode").toLowerCase(), "wal", "main WAL");
  assertEqual(textSql(profile.media, "PRAGMA journal_mode").toLowerCase(), "wal", "media WAL");
  assertEqual(
    numberSql(profile.main, "SELECT count(*) FROM refinery_schema_history"),
    migrationCount("main"),
    "main migration count",
  );
  assertEqual(
    numberSql(profile.media, "SELECT count(*) FROM refinery_schema_history"),
    migrationCount("media"),
    "media migration count",
  );
  assertEqual(numberSql(profile.main, "PRAGMA foreign_key_check"), 0, "main foreign keys");
  assertEqual(numberSql(profile.media, "PRAGMA foreign_key_check"), 0, "media foreign keys");
}

function logicalEvidence(profile) {
  return {
    activeTidbits: numberSql(profile.main, "SELECT count(*) FROM tidbit WHERE deleted_at IS NULL"),
    revisions: numberSql(profile.main, "SELECT count(*) FROM tidbit_revision"),
    sources: numberSql(profile.main, "SELECT count(*) FROM source"),
    attachments: numberSql(profile.main, "SELECT count(*) FROM attachment"),
    passages: numberSql(profile.main, "SELECT count(*) FROM passage"),
    searchDocuments: numberSql(profile.main, "SELECT count(*) FROM passage_search_document"),
    embeddings: numberSql(profile.main, "SELECT count(*) FROM passage_embedding"),
    legacyResearchRows: numberSql(profile.main, "SELECT count(*) FROM research_run"),
  };
}

function migrationSnapshot(profile, expectedMainHead, expectedMediaHead) {
  const root = join(profile.data, "safety-snapshots");
  return selectMigrationSnapshot({
    root,
    expectedMainHead,
    expectedMediaHead,
    inspect(snapshot) {
      verifySnapshot(snapshot);
      return {
        main: numberSql(snapshot.main, "SELECT max(version) FROM refinery_schema_history"),
        media: numberSql(snapshot.media, "SELECT max(version) FROM refinery_schema_history"),
      };
    },
  });
}

function verifySnapshot(snapshot) {
  const manifest = readJson(snapshot.manifest);
  assertEqual(manifest.schemaVersion, 1, "snapshot schema");
  assertEqual(manifest.reason, "migration", "snapshot reason");
  assertEqual(manifest.id, basename(snapshot.directory), "snapshot directory ownership");
  for (const [key, path] of [
    ["main", snapshot.main],
    ["media", snapshot.media],
  ]) {
    assertRegularFile(path, `snapshot ${key}`);
    assertEqual(lstatSync(path).size, manifest[key].bytes, `${key} snapshot bytes`);
    assertEqual(sha256File(path), manifest[key].sha256, `${key} snapshot SHA-256`);
    assertEqual(textSql(path, "PRAGMA integrity_check"), "ok", `${key} snapshot integrity`);
  }
}

function materializeSql(path, sql) {
  const result = spawnSync("sqlite3", ["-batch", path], {
    encoding: "utf8",
    input: sql,
  });
  requireSuccess(result, `materialize ${basename(path)}`);
}

function migrationCount(kind) {
  return readdirSync(join(appRoot, `src-tauri/src/database/migrations/${kind}`)).filter((name) =>
    /^V\d+__.+\.sql$/u.test(name),
  ).length;
}

function numberSql(database, statement) {
  const value = Number(textSql(database, statement));
  assert(Number.isSafeInteger(value), `SQL did not return an integer: ${statement}`);
  return value;
}

function textSql(database, statement) {
  const result = spawnSync("sqlite3", ["-batch", "-noheader", database, statement], {
    encoding: "utf8",
  });
  requireSuccess(result, `SQLite query: ${statement}`);
  return result.stdout.trim();
}

function assertStopped(profile) {
  const recordPath = join(profile.root, "launch.json");
  if (!existsSync(recordPath)) return;
  const record = readJson(recordPath);
  const result = spawnSync("ps", ["-p", String(record.pid), "-o", "command="], {
    encoding: "utf8",
  });
  if (result.status === 0 && result.stdout.trim()) {
    assert(
      !result.stdout.includes(record.executable),
      `packaged Kosh pid ${record.pid} is still running; quit with Cmd+Q`,
    );
  }
}

function packagedApp(argument) {
  const path = realpathSync(resolve(argument ?? defaultApp));
  assert(lstatSync(path).isDirectory(), `${path} is not a Kosh.app directory`);
  const executable = join(path, "Contents/MacOS/kosh");
  assertRegularFile(executable, "packaged Kosh executable");
  return { path, executable };
}

function profilePath(name) {
  assert(
    /^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$/u.test(name ?? ""),
    "profile name must contain only letters, digits, dot, underscore, and hyphen",
  );
  const path = resolve(acceptanceRoot, name);
  assert(
    path.startsWith(`${acceptanceRoot}${sep}`),
    "profile must be a direct acceptance-root child",
  );
  return path;
}

function newProfile(name) {
  const profile = profileFor(name);
  assert(!existsSync(profile.root), `acceptance profile already exists: ${profile.root}`);
  return profile;
}

function existingProfile(name) {
  const profile = profileFor(name);
  assert(
    existsSync(profile.root) && lstatSync(profile.root).isDirectory(),
    `acceptance profile does not exist: ${profile.root}`,
  );
  return profile;
}

function profileFor(name) {
  const root = profilePath(name);
  const home = join(root, "home");
  const data = join(home, "Library/Application Support/com.rohan.kosh");
  return {
    root,
    home,
    temporary: join(root, "tmp"),
    data,
    main: join(data, "kosh.sqlite3"),
    media: join(data, "media.sqlite3"),
  };
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  });
}

function writeJsonReplacing(path, value) {
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  });
  renameSync(temporary, path);
}

function requireSuccess(result, label) {
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`${label} failed with status ${result.status}`);
  }
}

function assertRegularFile(path, label) {
  const metadata = lstatSync(path);
  assert(metadata.isFile(), `${label} is not a regular file`);
  assert(!metadata.isSymbolicLink(), `${label} must not be a symlink`);
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

function printUsage() {
  console.info(`Usage:
  pnpm release:acceptance prepare-clean <profile>
  pnpm release:acceptance launch <profile> [Kosh.app]
  pnpm release:acceptance launch-hidden <profile> <absent|present> [Kosh.app]
  pnpm release:acceptance check-core <profile>
  pnpm release:acceptance check-journeys <profile>
  pnpm release:acceptance checkpoint-restart <profile>
  pnpm release:acceptance check-restart <profile>
  pnpm release:acceptance prepare-upgrade <profile>
  pnpm release:acceptance check-upgrade <profile>
  pnpm release:acceptance prove-upgrade-recovery <source> <new-target>`);
}
