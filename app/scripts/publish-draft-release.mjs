import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { basename, resolve } from "node:path";

import { assert, run } from "./distribution-signing.mjs";
import { readReleaseVersion } from "./release-version.mjs";

const [notesArgument] = process.argv.slice(2);
assert(notesArgument, "usage: node scripts/publish-draft-release.mjs <release-notes.md>");

const notesPath = resolve(notesArgument);
assert(existsSync(notesPath), `release notes are missing: ${notesPath}`);

const version = readReleaseVersion();
const tag = `v${version}`;
const releaseRoot = resolve("src-tauri/target/universal-apple-darwin/release/bundle/release");
const artifactPaths = [
  resolve(releaseRoot, `Kosh_${version}_universal.dmg`),
  resolve(releaseRoot, `Kosh_${version}_universal.app.tar.gz`),
  resolve(releaseRoot, `Kosh_${version}_universal.app.tar.gz.sig`),
  resolve(releaseRoot, "latest.json"),
  resolve(releaseRoot, "SHA256SUMS"),
];

for (const artifactPath of artifactPaths) {
  assert(existsSync(artifactPath), `release artifact is missing: ${artifactPath}`);
}

assert(
  run("git", ["status", "--porcelain"], { capture: true }).length === 0,
  "draft releases must be created from a clean worktree",
);
assert(
  run("git", ["branch", "--show-current"], { capture: true }) === "main",
  "draft releases must be created from main",
);
const sourceCommit = run("git", ["rev-parse", "HEAD"], { capture: true });
const remoteMain = run("git", ["ls-remote", "--heads", "origin", "refs/heads/main"], {
  capture: true,
}).split(/\s/u)[0];
assert(
  sourceCommit === remoteMain,
  "main must match the live origin/main before creating a draft release",
);
verifyReleaseMetadata(releaseRoot, version, sourceCommit);
assert(
  run("git", ["tag", "--list", tag], { capture: true }) === tag,
  `${tag} must exist locally before creating the draft release`,
);
assert(
  run("git", ["cat-file", "-t", `refs/tags/${tag}`], { capture: true }) === "tag",
  `${tag} must be an annotated local tag`,
);
assert(
  run("git", ["rev-list", "-n", "1", tag], { capture: true }) === sourceCommit,
  `${tag} must point to the current main commit`,
);
const remoteTag = run("git", ["ls-remote", "--tags", "origin", `refs/tags/${tag}^{}`], {
  capture: true,
}).split(/\s/u)[0];
assert(remoteTag === sourceCommit, `${tag} must be pushed to origin at the current main commit`);

run(
  "gh",
  [
    "release",
    "create",
    tag,
    "--draft",
    "--verify-tag",
    "--title",
    `Kosh ${version}`,
    "--notes-file",
    notesPath,
    ...artifactPaths,
  ],
  { stdio: "inherit" },
);

console.info(`Draft ${tag} created with ${artifactPaths.map(basename).join(", ")}.`);

function verifyReleaseMetadata(root, expectedVersion, expectedSourceCommit) {
  const archiveName = `Kosh_${expectedVersion}_universal.app.tar.gz`;
  const signature = readFileSync(resolve(root, `${archiveName}.sig`), "utf8").trim();
  const manifest = JSON.parse(readFileSync(resolve(root, "latest.json"), "utf8"));
  const armPlatform = manifest.platforms?.["darwin-aarch64"];
  const intelPlatform = manifest.platforms?.["darwin-x86_64"];
  assert(manifest.version === expectedVersion, "latest.json has the wrong version");
  assert(
    manifest.source?.commit === expectedSourceCommit,
    "release artifacts were built from a different source commit",
  );
  assert(manifest.source?.dirty === false, "release artifacts were built from a dirty worktree");
  for (const [architecture, platform] of [
    ["Apple Silicon", armPlatform],
    ["Intel", intelPlatform],
  ]) {
    assert(
      platform?.signature === signature,
      `latest.json has the wrong ${architecture} signature`,
    );
    assert(
      platform?.url ===
        `https://github.com/polyphilz/kosh/releases/download/v${expectedVersion}/${archiveName}`,
      `latest.json has the wrong ${architecture} updater URL`,
    );
  }

  const expectedChecksums = new Map(
    readFileSync(resolve(root, "SHA256SUMS"), "utf8")
      .trim()
      .split(/\r?\n/u)
      .map((line) => {
        const match = line.match(/^([a-f0-9]{64})  (.+)$/u);
        assert(match, `invalid SHA256SUMS line: ${line}`);
        return [match[2], match[1]];
      }),
  );
  for (const name of [
    `Kosh_${expectedVersion}_universal.dmg`,
    archiveName,
    `${archiveName}.sig`,
    "latest.json",
  ]) {
    assert(expectedChecksums.has(name), `SHA256SUMS omits ${name}`);
    assert(
      expectedChecksums.get(name) === sha256File(resolve(root, name)),
      `SHA256SUMS does not match ${name}`,
    );
  }
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}
