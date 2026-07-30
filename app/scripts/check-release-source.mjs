import { realpathSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const expectedRepository = realpathSync(
  resolve(process.argv[2] ?? import.meta.dirname, process.argv[2] ? "." : "../.."),
);
const repository = realpathSync(run("git", ["rev-parse", "--show-toplevel"], expectedRepository));
assertEqual(repository, expectedRepository, "release repository root");

const headSha = run("git", ["rev-parse", "HEAD"], repository);
assert(/^[0-9a-f]{40}$/u.test(headSha), "release source requires a full lowercase Git HEAD");
assert(
  run("git", ["status", "--porcelain=v1", "--untracked-files=normal"], repository) === "",
  "release source is dirty; commit or remove every tracked, staged, and untracked source change",
);

console.info(`Release source passed: clean exact HEAD ${headSha}.`);

function run(command, arguments_, cwd) {
  const result = spawnSync(command, arguments_, { cwd, encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }
  return result.stdout.trim();
}

function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`Unexpected ${label}: ${JSON.stringify(actual)}`);
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
