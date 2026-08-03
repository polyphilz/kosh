import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const appRoot = resolve(import.meta.dirname, "..");
const repositoryRoot = resolve(appRoot, "..");
const sourceRevision = gitOutput(["rev-parse", "HEAD"]).trim();
const stages = [
  ["check", "complete check suite"],
  ["relevance:gate", "search relevance gate"],
  ["baseline:redesign", "redesign performance baseline"],
  ["check:bundle", "production bundle isolation"],
  ["release:migration", "hard-cutover migration contract"],
];

assertExactSource("before redesign acceptance");
for (const [script, label] of stages) {
  assertExactSource(`before ${label}`);
  run("pnpm", [script], label);
  assertExactSource(`after ${label}`);
}

console.info(`Redesign acceptance passed for ${sourceRevision}.`);

function assertExactSource(context) {
  const revision = gitOutput(["rev-parse", "HEAD"]).trim();
  if (revision !== sourceRevision) {
    throw new Error(`${context}: expected source revision ${sourceRevision}, found ${revision}`);
  }
  const status = gitOutput(["status", "--porcelain=v1", "--untracked-files=all"]);
  if (status.trim().length > 0) {
    throw new Error(`${context}: source tree is not clean:\n${status}`);
  }
}

function gitOutput(arguments_) {
  const result = spawnSync("git", ["-C", repositoryRoot, ...arguments_], {
    encoding: "utf8",
  });
  requireSuccess(result, `git ${arguments_.join(" ")}`);
  return result.stdout;
}

function run(command, arguments_, label) {
  const result = spawnSync(command, arguments_, {
    cwd: appRoot,
    env: { ...process.env, KOSH_ACCEPTANCE_GIT_SHA: sourceRevision },
    stdio: "inherit",
  });
  requireSuccess(result, label);
}

function requireSuccess(result, label) {
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const outcome = result.signal ? `signal ${result.signal}` : `exit ${result.status}`;
    throw new Error(`${label} failed with ${outcome}`);
  }
}
