import { appendFileSync, mkdtempSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";

const checker = resolve(import.meta.dirname, "check-release-source.mjs");
const ownedRoot = mkdtempSync(join(tmpdir(), "kosh-release-source-test."));
if (
  !ownedRoot.startsWith(`${tmpdir()}${sep}`) ||
  !ownedRoot.split(sep).at(-1)?.startsWith("kosh-release-source-test.")
) {
  throw new Error(`refusing to use unexpected release-source test root: ${ownedRoot}`);
}

try {
  run("git", ["init", "--quiet"], ownedRoot);
  run("git", ["config", "user.email", "kosh-release-test@example.invalid"], ownedRoot);
  run("git", ["config", "user.name", "Kosh release test"], ownedRoot);
  writeFileSync(join(ownedRoot, ".gitignore"), "ignored.env\n");
  writeFileSync(join(ownedRoot, "tracked.txt"), "clean\n");
  run("git", ["add", ".gitignore", "tracked.txt"], ownedRoot);
  run("git", ["commit", "--quiet", "-m", "baseline"], ownedRoot);
  expectAccepted("clean checkout");

  appendFileSync(join(ownedRoot, "tracked.txt"), "dirty\n");
  expectRejected("modified tracked source");
  run("git", ["restore", "tracked.txt"], ownedRoot);

  appendFileSync(join(ownedRoot, "tracked.txt"), "staged\n");
  run("git", ["add", "tracked.txt"], ownedRoot);
  expectRejected("staged source");
  run("git", ["restore", "--staged", "--worktree", "tracked.txt"], ownedRoot);

  writeFileSync(join(ownedRoot, "untracked.txt"), "untracked\n");
  expectRejected("untracked source");
  unlinkSync(join(ownedRoot, "untracked.txt"));

  writeFileSync(join(ownedRoot, "ignored.env"), "local-only\n");
  expectAccepted("ignored local data");

  console.info("release source guard tests passed");
} finally {
  rmSync(ownedRoot, { recursive: true });
}

function expectAccepted(label) {
  const result = checkerResult();
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`release source guard rejected ${label}`);
  }
}

function expectRejected(label) {
  const result = checkerResult();
  if (result.status === 0) {
    throw new Error(`release source guard accepted ${label}`);
  }
  if (!result.stderr.includes("release source is dirty")) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`release source guard rejected ${label} for the wrong reason`);
  }
}

function checkerResult() {
  return spawnSync(process.execPath, [checker, ownedRoot], { encoding: "utf8" });
}

function run(command, arguments_, cwd) {
  const result = spawnSync(command, arguments_, { cwd, encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`${command} ${arguments_.join(" ")} failed`);
  }
}
