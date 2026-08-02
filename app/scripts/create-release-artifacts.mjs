import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

import { assert, assertEqual, run } from "./distribution-signing.mjs";
import { readReleaseVersion } from "./release-version.mjs";
import {
  preflightUpdaterCredentials,
  signUpdaterArchive,
  takeUpdaterEnvironment,
  verifyUpdaterArchiveSignature,
} from "./updater-signing.mjs";

const releaseRoot = resolve("src-tauri/target/universal-apple-darwin/release/bundle/release");
const tauriConfigPath = resolve("src-tauri/tauri.conf.json");

const scriptArguments = process.argv.slice(2);
if (scriptArguments[0] === "--") {
  scriptArguments.shift();
}
assert(
  scriptArguments.length === 0 || scriptArguments.length === 2,
  "usage: node scripts/create-release-artifacts.mjs [<Kosh.app> <Kosh.dmg>]",
);

const version = readReleaseVersion();
const [applicationArgument, diskImageArgument] = scriptArguments;
const applicationPath = applicationArgument
  ? resolve(applicationArgument)
  : resolve("src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app");
const diskImagePath = diskImageArgument
  ? resolve(diskImageArgument)
  : resolve(
      `src-tauri/target/universal-apple-darwin/release/bundle/dmg/Kosh_${version}_universal.dmg`,
    );
const updaterCredentials = takeUpdaterEnvironment();

preflight(applicationPath, diskImagePath, updaterCredentials);
run("node", ["scripts/check-notarized-distribution.mjs", applicationPath, diskImagePath], {
  stdio: "inherit",
});
rmSync(releaseRoot, { recursive: true, force: true });
mkdirSync(releaseRoot, { recursive: true });

const diskImageName = `Kosh_${version}_universal.dmg`;
const updaterArchiveName = `Kosh_${version}_universal.app.tar.gz`;
const diskImageReleasePath = resolve(releaseRoot, diskImageName);
const updaterArchivePath = resolve(releaseRoot, updaterArchiveName);
const updaterSignaturePath = `${updaterArchivePath}.sig`;
const manifestPath = resolve(releaseRoot, "latest.json");
const checksumsPath = resolve(releaseRoot, "SHA256SUMS");

copyFileSync(diskImagePath, diskImageReleasePath);
createUpdaterArchive(applicationPath, updaterArchivePath);
signUpdaterArchive(updaterArchivePath, updaterCredentials);
verifyUpdaterArchiveSignature(updaterArchivePath, updaterSignaturePath, tauriConfigPath);

const signature = readFileSync(updaterSignaturePath, "utf8").trim();
assert(signature.length > 0, "the updater signature is empty");
const source = readPackagedSourceProvenance(applicationPath);
writeFileSync(
  manifestPath,
  `${JSON.stringify(
    {
      version,
      notes: `See the Kosh v${version} release notes on GitHub.`,
      pub_date: new Date().toISOString(),
      source,
      platforms: {
        "darwin-aarch64": {
          signature,
          url:
            `https://github.com/polyphilz/kosh/releases/download/` +
            `v${version}/${updaterArchiveName}`,
        },
        "darwin-x86_64": {
          signature,
          url:
            `https://github.com/polyphilz/kosh/releases/download/` +
            `v${version}/${updaterArchiveName}`,
        },
      },
    },
    null,
    2,
  )}\n`,
);

const releaseFiles = [diskImageReleasePath, updaterArchivePath, updaterSignaturePath, manifestPath];
writeFileSync(
  checksumsPath,
  `${releaseFiles.map((path) => `${sha256File(path)}  ${basename(path)}`).join("\n")}\n`,
);

console.info(`Release artifacts passed: ${releaseRoot}`);

function preflight(applicationPath_, diskImagePath_, credentials) {
  assert(process.platform === "darwin", "release artifacts require macOS");
  assert(existsSync(applicationPath_), `application is missing: ${applicationPath_}`);
  assert(existsSync(diskImagePath_), `disk image is missing: ${diskImagePath_}`);
  preflightUpdaterCredentials(credentials);
  assertEqual(
    run(
      "/usr/libexec/PlistBuddy",
      ["-c", "Print :CFBundleShortVersionString", resolve(applicationPath_, "Contents/Info.plist")],
      { capture: true },
    ),
    version,
    "packaged application version",
  );
}

function createUpdaterArchive(applicationPath_, archivePath) {
  mkdirSync(dirname(archivePath), { recursive: true });
  rmSync(archivePath, { force: true });
  rmSync(`${archivePath}.sig`, { force: true });
  const archiveEnvironment = {
    ...process.env,
    COPYFILE_DISABLE: "1",
  };
  run(
    "/usr/bin/tar",
    [
      "--no-mac-metadata",
      "-czf",
      archivePath,
      "-C",
      dirname(applicationPath_),
      basename(applicationPath_),
    ],
    { env: archiveEnvironment, stdio: "inherit" },
  );
  const entries = run("/usr/bin/tar", ["-tzf", archivePath], {
    capture: true,
    env: archiveEnvironment,
  })
    .split(/\r?\n/u)
    .filter(Boolean);
  const root = `${basename(applicationPath_)}/`;
  assert(entries.length > 1, "the updater archive is empty");
  assert(
    entries.every((entry) => entry === basename(applicationPath_) || entry.startsWith(root)),
    "the updater archive contains a path outside Kosh.app",
  );
  assert(
    entries.every((entry) => !entry.split("/").some((component) => component.startsWith("._"))),
    "the updater archive contains macOS AppleDouble metadata",
  );
  verifyExtractedUpdaterArchive(archivePath, applicationPath_, archiveEnvironment);
}

function verifyExtractedUpdaterArchive(archivePath, applicationPath_, archiveEnvironment) {
  const extractionDirectory = mkdtempSync(join(tmpdir(), "kosh-updater-extraction-"));
  const extractedApplicationPath = join(extractionDirectory, basename(applicationPath_));
  try {
    run("/usr/bin/tar", ["--no-mac-metadata", "-xzf", archivePath, "-C", extractionDirectory], {
      env: archiveEnvironment,
      stdio: "inherit",
    });
    assert(existsSync(extractedApplicationPath), "the updater archive did not extract Kosh.app");
    run(
      "/usr/bin/codesign",
      ["--verify", "--deep", "--strict", "--verbose=2", extractedApplicationPath],
      { stdio: "inherit" },
    );
    run(
      "/usr/sbin/spctl",
      ["--assess", "--type", "execute", "--verbose=4", extractedApplicationPath],
      { stdio: "inherit" },
    );
  } finally {
    rmSync(extractionDirectory, { recursive: true, force: true });
  }
}

function readPackagedSourceProvenance(applicationPath_) {
  const provenancePath = resolve(applicationPath_, "Contents/Resources/release/source.json");
  assert(existsSync(provenancePath), `packaged release provenance is missing: ${provenancePath}`);
  const provenance = JSON.parse(readFileSync(provenancePath, "utf8"));
  assert(/^[a-f0-9]{40}$/u.test(provenance.commit), "packaged source commit is invalid");
  assert(typeof provenance.dirty === "boolean", "packaged source dirty state is invalid");
  return provenance;
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}
