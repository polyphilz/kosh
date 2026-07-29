import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { basename, extname, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const pin = readJson("src-tauri/resources/sidecars/llama-server-v1.json");
const stage = resolve("src-tauri/resources/release");
const manifestPath = resolve(stage, pin.stagingPaths.releaseManifest);
const manifest = readJson(manifestPath);
const binary = resolve(stage, pin.stagingPaths.binary);
const license = resolve(stage, pin.stagingPaths.license);

assertEqual(stripGeneratedFields(manifest), pin, "release inputs");
assertEqual(manifest.verification.modelBundled, false, "model bundle policy");
assertEqual(
  manifest.verification.architectureChecks,
  pin.target.architectures.map((architecture) => ({
    architecture,
    cpuPassed: true,
    metalPassed: true,
  })),
  "CPU and Metal architecture verification",
);

assertRegularExecutable(binary, "staged llama-server");
assertEqual(lstatSync(binary).size, manifest.binary.size, "binary byte length");
assertEqual(sha256File(binary), manifest.binary.sha256, "binary SHA-256");
assertArchitectures(binary, pin.target.architectures);
assertSystemDependencies(binary, pin.target.architectures);
run("codesign", ["--verify", "--strict", "--verbose=2", binary]);

for (const architecture of pin.target.architectures) {
  assertEqual(
    run("arch", [`-${architecture}`, binary, "--version"]),
    manifest.binary.versionOutputByArchitecture[architecture],
    `${architecture} llama-server version`,
  );
}

assertEqual(
  sha256File("src-tauri/resources/embedding-indexes/jina-v1.json"),
  manifest.verification.embeddingManifest.sha256,
  "embedding manifest SHA-256",
);
assertEqual(
  sha256File("src-tauri/resources/embedding-indexes/jina-v1-golden.json"),
  manifest.verification.goldenFixtures.sha256,
  "golden fixtures SHA-256",
);
assertEqual(sha256File(license), manifest.licenseNotices[0].sha256, "license SHA-256");

const stagedFiles = listFiles(stage)
  .map((path) => path.slice(stage.length + 1).replaceAll("\\", "/"))
  .sort();
assertEqual(
  stagedFiles,
  ["bin/llama-server", "licenses/llama.cpp-LICENSE", "llama-server.json"],
  "staged release files",
);
for (const path of listFiles(stage)) {
  const metadata = lstatSync(path);
  assert(!metadata.isSymbolicLink(), `staged resource is a symlink: ${path}`);
  const normalized = path.toLowerCase();
  assert(extname(path) !== ".gguf", `model weights were staged: ${path}`);
  for (const prohibited of ["tests/", "playwright", "wdio", ".env"]) {
    assert(!normalized.includes(prohibited), `test or secret material was staged: ${path}`);
  }
}

console.info(
  `Release resources passed: universal ${pin.target.architectures.join("+")} llama-server ${manifest.binary.sha256} (${manifest.binary.size} bytes), CPU and Metal fixtures verified.`,
);

function stripGeneratedFields(value) {
  const normalized = structuredClone(value);
  delete normalized.binary;
  delete normalized.verification;
  normalized.licenseNotices = normalized.licenseNotices.map((notice) => {
    delete notice.sha256;
    return notice;
  });
  return normalized;
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
