import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const appRoot = resolve(import.meta.dirname, "..");
const matrixPath = resolve(appRoot, "fixtures/backup/fault-matrix-v1.json");
const reportPath = resolve(
  process.env.KOSH_BACKUP_FAULT_MATRIX_REPORT ??
    `${appRoot}/.data/backup-acceptance/fault-matrix-report-v1.json`,
);
const matrixBytes = readFileSync(matrixPath);
const matrix = JSON.parse(matrixBytes);

const sourceByModule = new Map([
  ["backup::checkpoint", "src-tauri/src/backup/checkpoint.rs"],
  ["backup::credentials", "src-tauri/src/backup/credentials.rs"],
  ["backup::litestream", "src-tauri/src/backup/litestream.rs"],
  ["backup::litestream_runtime", "src-tauri/src/backup/litestream_runtime.rs"],
  ["backup::media_reconciler", "src-tauri/src/backup/media_reconciler.rs"],
  ["backup::owner", "src-tauri/src/backup/owner.rs"],
  ["backup::recovery_cli", "src-tauri/src/backup/recovery_cli.rs"],
  ["backup::restore", "src-tauri/src/backup/restore.rs"],
  ["database::backup_media_tests", "src-tauri/src/database/backup_media_tests.rs"],
  ["database::backup_state_tests", "src-tauri/src/database/backup_state_tests.rs"],
  ["database::maintenance_tests", "src-tauri/src/database/maintenance_tests.rs"],
  ["database::offsite_checkpoint_tests", "src-tauri/src/database/offsite_checkpoint_tests.rs"],
  ["database::research_runs_tests", "src-tauri/src/database/research_runs_tests.rs"],
  ["database::restore_install", "src-tauri/src/database/restore_install.rs"],
  ["database::safety_snapshot", "src-tauri/src/database/safety_snapshot.rs"],
]);
const requiredPhases = [
  "snapshot",
  "configuration",
  "media",
  "replication",
  "checkpoint",
  "discovery",
  "restore",
  "install",
  "reopen",
];
const requiredFailureClasses = [
  "auth",
  "capacity",
  "clock",
  "concurrency",
  "corruption",
  "crash-boundary",
  "filesystem",
  "network",
  "process",
  "retry",
  "security",
  "stale-state",
];
const requiredInvariants = [
  "authored-bytes",
  "bounded-cleanup",
  "citation-provenance",
  "exact-txid",
  "idempotency",
  "local-availability",
  "media-immutability",
  "search-rebuildability",
  "secret-isolation",
  "single-writer",
];

assert(matrix.schemaVersion === 1, "fault matrix schema must be 1");
assert(Array.isArray(matrix.cases) && matrix.cases.length >= 40, "fault matrix is incomplete");
const ids = new Set();
const tests = new Set();
const phases = new Set();
const failureClasses = new Set();
const invariants = new Set();
const sourceCache = new Map();

for (const entry of matrix.cases) {
  assertObject(entry, "matrix case");
  assertToken(entry.id, "case id");
  assertToken(entry.phase, `${entry.id} phase`);
  assertToken(entry.failureClass, `${entry.id} failure class`);
  assert(typeof entry.test === "string", `${entry.id} has no test`);
  assert(
    Array.isArray(entry.invariants) && entry.invariants.length > 0,
    `${entry.id} has no invariants`,
  );
  assert(!ids.has(entry.id), `duplicate case id: ${entry.id}`);
  assert(!tests.has(entry.test), `test mapped more than once: ${entry.test}`);
  ids.add(entry.id);
  tests.add(entry.test);
  phases.add(entry.phase);
  failureClasses.add(entry.failureClass);
  for (const invariant of entry.invariants) {
    assertToken(invariant, `${entry.id} invariant`);
    invariants.add(invariant);
  }
  verifyTest(entry.test);
}

for (const [label, required, actual] of [
  ["phase", requiredPhases, phases],
  ["failure class", requiredFailureClasses, failureClasses],
  ["invariant", requiredInvariants, invariants],
]) {
  for (const value of required) {
    assert(actual.has(value), `fault matrix is missing required ${label}: ${value}`);
  }
}

const countsByPhase = Object.fromEntries(
  [...phases]
    .sort()
    .map((phase) => [phase, matrix.cases.filter((entry) => entry.phase === phase).length]),
);
const report = {
  schemaVersion: 1,
  result: "pass",
  matrixSha256: createHash("sha256").update(matrixBytes).digest("hex"),
  caseCount: matrix.cases.length,
  countsByPhase,
  failureClasses: [...failureClasses].sort(),
  invariants: [...invariants].sort(),
  tests: [...tests].sort(),
};
mkdirSync(dirname(reportPath), { recursive: true });
writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
console.info(
  `Backup fault matrix passed: ${matrix.cases.length} deterministic cases across ${phases.size} phases.`,
);

function verifyTest(test) {
  const module = [...sourceByModule.keys()]
    .filter((candidate) => test.startsWith(`${candidate}::`))
    .sort((left, right) => right.length - left.length)[0];
  assert(module, `unmapped Rust test module: ${test}`);
  const suffix = test.slice(module.length + 2);
  const testName = suffix.startsWith("tests::") ? suffix.slice("tests::".length) : suffix;
  assert(/^[a-z][a-z0-9_]+$/u.test(testName), `invalid Rust test name: ${test}`);
  const relativeSource = sourceByModule.get(module);
  assert(relativeSource, `unmapped Rust test source: ${module}`);
  let source = sourceCache.get(relativeSource);
  if (!source) {
    source = readFileSync(resolve(appRoot, relativeSource), "utf8");
    sourceCache.set(relativeSource, source);
  }
  const escaped = testName.replaceAll(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const pattern = new RegExp(
    String.raw`#\[(?:tokio::)?test\][\s\S]{0,240}\bfn\s+${escaped}\s*\(`,
    "u",
  );
  assert(pattern.test(source), `mapped Rust test does not exist: ${test}`);
}

function assertObject(value, label) {
  assert(value !== null && typeof value === "object" && !Array.isArray(value), `${label} invalid`);
}

function assertToken(value, label) {
  assert(typeof value === "string" && /^[a-z][a-z0-9-]*$/u.test(value), `${label} invalid`);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
