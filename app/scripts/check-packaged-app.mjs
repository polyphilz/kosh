import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { basename, extname, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const appPath = resolve(
  process.argv[2] ?? "src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app",
);
const resources = resolve(appPath, "Contents/Resources");
const appBinary = resolve(appPath, "Contents/MacOS/kosh");
const sidecar = resolve(resources, "bin/llama-server");
const litestream = resolve(resources, "bin/litestream");
const releaseManifestPath = resolve(resources, "release/llama-server.json");
const manifest = readJson(releaseManifestPath);
const pin = readJson("src-tauri/resources/sidecars/llama-server-v1.json");
const litestreamManifest = readJson(resolve(resources, "release/litestream.json"));
const litestreamPin = readJson("src-tauri/resources/sidecars/litestream-v1.json");
const packageJson = readJson("package.json");
const infoPlist = resolve(appPath, "Contents/Info.plist");

assertDirectory(appPath, "Kosh.app");
assertRegularExecutable(appBinary, "Kosh executable");
assertRegularExecutable(sidecar, "bundled llama-server");
assertRegularExecutable(litestream, "bundled Litestream");
assertEqual(sha256File(sidecar), manifest.binary.sha256, "bundled llama-server SHA-256");
assertEqual(
  sha256File("src-tauri/resources/release/bin/llama-server"),
  manifest.binary.sha256,
  "staged llama-server SHA-256",
);
assertEqual(lstatSync(sidecar).size, manifest.binary.size, "bundled llama-server size");
assertEqual(
  sha256File(resolve(resources, "embedding-indexes/jina-v1.json")),
  manifest.verification.embeddingManifest.sha256,
  "bundled embedding manifest SHA-256",
);
assertEqual(
  sha256File(resolve(resources, "embedding-indexes/jina-v1-golden.json")),
  manifest.verification.goldenFixtures.sha256,
  "bundled golden fixtures SHA-256",
);
assertEqual(
  sha256File(resolve(resources, manifest.licenseNotices[0].bundlePath)),
  manifest.licenseNotices[0].sha256,
  "bundled license SHA-256",
);
assertEqual(
  sha256File(litestream),
  litestreamPin.binary.universal.sha256,
  "bundled Litestream SHA-256",
);
assertEqual(
  sha256File("src-tauri/resources/release/bin/litestream"),
  litestreamPin.binary.universal.sha256,
  "staged Litestream SHA-256",
);
assertEqual(
  lstatSync(litestream).size,
  litestreamPin.binary.universal.size,
  "bundled Litestream size",
);
assertEqual(
  litestreamManifest.stagedBinary.sha256,
  litestreamPin.binary.universal.sha256,
  "bundled Litestream manifest SHA-256",
);
assertEqual(
  sha256File(resolve(resources, litestreamPin.licenseNotices[0].bundlePath)),
  litestreamPin.licenseNotices[0].sha256,
  "bundled Litestream license SHA-256",
);
assertEqual(
  sha256File(resolve(resources, litestreamPin.resourceDestinations.notice)),
  sha256File("src-tauri/resources/sidecars/litestream-NOTICE"),
  "bundled Litestream notice SHA-256",
);

for (const binary of [appBinary, sidecar, litestream]) {
  assertArchitectures(binary, pin.target.architectures);
}
assertSystemDependencies(sidecar, pin.target.architectures);
assertSystemDependencies(litestream, litestreamPin.target.architectures);
for (const architecture of pin.target.architectures) {
  assertEqual(
    run("arch", [`-${architecture}`, sidecar, "--version"]),
    manifest.binary.versionOutputByArchitecture[architecture],
    `bundled ${architecture} llama-server version`,
  );
}
for (const architecture of litestreamPin.target.architectures) {
  assertEqual(
    run("arch", [`-${architecture}`, litestream, "version"]),
    litestreamPin.binary.versionOutputByArchitecture[architecture],
    `bundled ${architecture} Litestream version`,
  );
}

run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", appPath]);
run("codesign", ["--verify", "--strict", "--verbose=2", sidecar]);
run("codesign", ["--verify", "--strict", "--verbose=2", litestream]);
const signature = run("codesign", ["-dv", "--verbose=4", appPath]);
assertEqual(
  signatureField(signature, "Identifier"),
  "com.rohan.kosh",
  "signed application identifier",
);
assertEqual(signatureField(signature, "Signature"), "adhoc", "app signature");
assert(!signature.includes("runtime"), "personal-v1 package unexpectedly enables hardened runtime");

assertEqual(plist("CFBundleIdentifier"), "com.rohan.kosh", "packaged application identifier");
assertEqual(plist("CFBundleName"), "Kosh", "packaged application name");
assertEqual(
  plist("CFBundleShortVersionString"),
  packageJson.version,
  "packaged application version",
);
assertEqual(
  plist("LSMinimumSystemVersion"),
  pin.target.minimumSystemVersion,
  "packaged minimum macOS version",
);

const expectedResources = [
  "bin/litestream",
  "bin/llama-server",
  "embedding-indexes/jina-v1-golden.json",
  "embedding-indexes/jina-v1.json",
  "icon.icns",
  "licenses/litestream-LICENSE",
  "licenses/litestream-NOTICE",
  "licenses/llama.cpp-LICENSE",
  "release/litestream.json",
  "release/llama-server.json",
].sort();
const packagedResources = listFiles(resources)
  .map((path) => path.slice(resources.length + 1).replaceAll("\\", "/"))
  .sort();
assertEqual(packagedResources, expectedResources, "packaged resource files");

for (const path of listFiles(appPath)) {
  const metadata = lstatSync(path);
  assert(!metadata.isSymbolicLink(), `packaged file is a symlink: ${path}`);
  const normalized = path.toLowerCase();
  assert(extname(path) !== ".gguf", `model weights were bundled: ${path}`);
  for (const prohibited of ["tests/", "playwright", "wdio", ".env", ".plans", ".data/"]) {
    assert(
      !normalized.includes(prohibited),
      `test, plan, data, or secret material was bundled: ${path}`,
    );
  }
}

const quarantine = spawnSync("xattr", ["-p", "com.apple.quarantine", appPath], {
  encoding: "utf8",
});
assert(quarantine.status !== 0, "locally built Kosh.app unexpectedly has a quarantine attribute");

console.info(
  `Packaged app passed: ${appPath}, ad-hoc signed universal ${pin.target.architectures.join("+")} macOS ${pin.target.minimumSystemVersion}+, pinned llama-server ${manifest.binary.sha256} and Litestream ${litestreamPin.binary.universal.sha256}.`,
);

function plist(key) {
  return run("/usr/libexec/PlistBuddy", ["-c", `Print :${key}`, infoPlist]);
}

function assertDirectory(path, label) {
  const metadata = lstatSync(path);
  assert(metadata.isDirectory(), `${label} is not a directory`);
  assert(!metadata.isSymbolicLink(), `${label} must not be a symlink`);
}

function assertRegularExecutable(path, label) {
  const metadata = lstatSync(path);
  assert(metadata.isFile(), `${label} is not a regular file`);
  assert(!metadata.isSymbolicLink(), `${label} must not be a symlink`);
  assert((metadata.mode & 0o111) !== 0, `${label} is not executable`);
}

function assertArchitectures(path, expected) {
  const actual = run("lipo", ["-archs", path]).split(/\s+/u).sort();
  assertEqual(actual, [...expected].sort(), `${basename(path)} architectures`);
}

function assertSystemDependencies(path, architectures) {
  for (const architecture of architectures) {
    const dependencies = run("otool", ["-arch", architecture, "-L", path])
      .split(/\r?\n/u)
      .slice(1)
      .map((line) => line.trim())
      .filter(Boolean);
    const nonSystem = dependencies.find(
      (dependency) =>
        !dependency.startsWith("/System/Library/") && !dependency.startsWith("/usr/lib/"),
    );
    assert(
      !nonSystem,
      `${basename(path)} ${architecture} has a non-system dependency: ${nonSystem}`,
    );
  }
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function listFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    return entry.isDirectory() ? listFiles(path) : [path];
  });
}

function signatureField(signature, field) {
  const prefix = `${field}=`;
  const values = signature
    .split(/\r?\n/u)
    .filter((line) => line.startsWith(prefix))
    .map((line) => line.slice(prefix.length));
  assertEqual(values.length, 1, `${field} signature field count`);
  return values[0];
}

function run(command, arguments_) {
  const result = spawnSync(command, arguments_, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }
  return `${result.stdout}${result.stderr}`.trim();
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
