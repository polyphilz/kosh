import { lstatSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const local = readJson("src-tauri/tauri.conf.json");
const release = readJson("src-tauri/tauri.release.conf.json");
const packageJson = readJson("package.json");
const pin = readJson("src-tauri/resources/sidecars/llama-server-v1.json");
const litestreamPin = readJson("src-tauri/resources/sidecars/litestream-v1.json");
const distributionPolicy = readJson("src-tauri/distribution-signing.json");
const cargo = readFileSync("src-tauri/Cargo.toml", "utf8");
const main = readFileSync("src-tauri/src/main.rs", "utf8");
const distributionBuild = readFileSync("scripts/build-notarized-distribution.mjs", "utf8");
const releaseArtifacts = readFileSync("scripts/create-release-artifacts.mjs", "utf8");

assertEqual(local.productName, "Kosh", "product name");
assertEqual(local.identifier, "com.rohan.kosh", "bundle identifier");
assertEqual(local.version, packageJson.version, "Tauri/package version");
assertEqual(
  cargo.match(/^\s*version\s*=\s*"([^"]+)"/mu)?.[1],
  packageJson.version,
  "Cargo/package version",
);
assertEqual(local.bundle, { active: false }, "development bundle policy");
assertEqual(local.app.macOSPrivateApi, true, "macOS private API policy");
assertEqual(local.app.security.freezePrototype, true, "prototype freeze");
assertEqual(local.app.security.capabilities, ["default", "quick-add"], "capability allowlist");
assertEqual(
  local.plugins.updater.endpoints,
  ["https://github.com/polyphilz/kosh/releases/latest/download/latest.json"],
  "updater endpoint",
);
assert(
  typeof local.plugins.updater.pubkey === "string" && local.plugins.updater.pubkey.length > 80,
  "updater public key must be embedded",
);
assertEqual(
  local.app.security.csp,
  {
    "default-src": "'self'",
    "connect-src": "ipc: http://ipc.localhost",
    "img-src": "'self' blob: data: kosh-media:",
    "style-src": "'self' 'unsafe-inline'",
    "object-src": "kosh-media:",
    "frame-src": "'none'",
    "base-uri": "'none'",
    "form-action": "'none'",
  },
  "release CSP",
);

assertEqual(release.bundle.active, true, "release bundle activation");
assertEqual(release.bundle.targets, ["app"], "release bundle targets");
assertEqual(release.bundle.category, "Productivity", "application category");
assert(release.bundle.shortDescription.endsWith("."), "short description must be complete");
assert(
  release.bundle.longDescription.includes("local-first"),
  "long description must state the local-first policy",
);
assertEqual(
  release.bundle.macOS.minimumSystemVersion,
  pin.target.minimumSystemVersion,
  "minimum macOS version",
);
assertEqual(
  release.bundle.macOS.minimumSystemVersion,
  litestreamPin.target.minimumSystemVersion,
  "Litestream minimum macOS version",
);
assertEqual(release.bundle.macOS.signingIdentity, "-", "ad-hoc signing identity");
assertEqual(release.bundle.macOS.hardenedRuntime, false, "personal-v1 hardened-runtime policy");
assertEqual(release.bundle.macOS.entitlements, "Entitlements.plist", "entitlements path");
assertEqual(pin.target.architectures, ["arm64", "x86_64"], "universal sidecar architectures");

const expectedIcons = [
  "icons/32x32.png",
  "icons/128x128.png",
  "icons/128x128@2x.png",
  "icons/icon.icns",
];
assertEqual(release.bundle.icon, expectedIcons, "release icon set");
for (const icon of [...expectedIcons, "icons/icon.png", "icons/tray-icon.png"]) {
  assertRegularFile(resolve("src-tauri", icon), `icon ${icon}`);
}
assertEqual(
  readPngHeader("src-tauri/icons/tray-icon.png"),
  { width: 32, height: 32, bitDepth: 8, hasAlpha: true },
  "tray template dimensions",
);

const entitlementsPath = "src-tauri/Entitlements.plist";
const entitlementsSource = readFileSync(entitlementsPath, "utf8");
assert(
  /^<\?xml version="1\.0" encoding="UTF-8"\?>\s*<!DOCTYPE plist PUBLIC "-\/\/Apple\/\/DTD PLIST 1\.0\/\/EN" "http:\/\/www\.apple\.com\/DTDs\/PropertyList-1\.0\.dtd">\s*<plist version="1\.0">\s*<dict\/>\s*<\/plist>\s*$/u.test(
    entitlementsSource,
  ),
  "entitlements must be a canonical empty property list",
);
if (process.platform === "darwin") {
  assertEqual(
    JSON.parse(run("plutil", ["-convert", "json", "-o", "-", entitlementsPath])),
    {},
    "least-privilege entitlements",
  );
}

assertEqual(
  readJson("src-tauri/capabilities/default.json"),
  {
    $schema: "../gen/schemas/desktop-schema.json",
    identifier: "default",
    description: "Capability for the main Kosh window",
    windows: ["main"],
    permissions: ["core:default"],
  },
  "main-window capability",
);
assertEqual(
  readJson("src-tauri/capabilities/quick-add.json"),
  {
    $schema: "../gen/schemas/desktop-schema.json",
    identifier: "quick-add",
    description: "Capability for the persistent Kosh quick-add window",
    windows: ["quick-add"],
    permissions: ["core:default"],
  },
  "quick-add capability",
);
assertEqual(
  readJson("src-tauri/capabilities/main-updater.json"),
  {
    $schema: "../gen/schemas/desktop-schema.json",
    identifier: "main-updater",
    description: "Production-only updater access for Kosh's main window",
    windows: ["main"],
    platforms: ["macOS"],
    permissions: [
      "process:allow-restart",
      "updater:allow-check",
      "updater:allow-download-and-install",
    ],
  },
  "production updater capability",
);

assertEqual(distributionPolicy.formatVersion, 1, "distribution policy version");
assertEqual(
  distributionPolicy.application,
  {
    bundleIdentifier: "com.rohan.kosh",
    signingIdentity: "Developer ID Application: SILO77 LLC (PMZH6ULML8)",
    teamIdentifier: "PMZH6ULML8",
  },
  "distribution publisher identity",
);
assertEqual(
  Object.keys(distributionPolicy.sidecars).sort(),
  ["litestream", "llamaServer"],
  "distribution sidecars",
);
assertEqual(
  packageJson.scripts["release:build:distribution"],
  "node scripts/build-notarized-distribution.mjs",
  "distribution build command",
);
assertEqual(
  packageJson.scripts["release:publish:draft"],
  "node scripts/publish-draft-release.mjs",
  "draft release command",
);
assert(
  !packageJson.scripts["release:build:app"].includes("VITE_KOSH_UPDATER_ENABLED"),
  "local app builds must not enable the updater",
);
for (const contract of [
  'VITE_KOSH_UPDATER_ENABLED: "true"',
  'capabilities: ["default", "quick-add", "main-updater"]',
  '"universal-apple-darwin"',
  "hardenedRuntime: true",
]) {
  assert(distributionBuild.includes(contract), `distribution build omits ${contract}`);
}
const sourceVerification = distributionBuild.indexOf(
  'run("pnpm", ["release:verify-source"], { stdio: "inherit" });',
);
const signingVerification = distributionBuild.indexOf(
  'run("pnpm", ["release:verify-updater-signing"], { stdio: "inherit" });',
);
const distributionBuildStart = distributionBuild.indexOf("buildSignedApplication(policy);");
assert(sourceVerification >= 0, "distribution build omits clean-source verification");
assert(
  sourceVerification < signingVerification && sourceVerification < distributionBuildStart,
  "distribution build must verify clean source before signing preflight or build work",
);
for (const platform of ['"darwin-aarch64"', '"darwin-x86_64"']) {
  assert(releaseArtifacts.includes(platform), `updater manifest omits ${platform}`);
}

const expectedResources = {
  "resources/release/bin/llama-server": "bin/llama-server",
  "resources/release/bin/litestream": "bin/litestream",
  "resources/release/llama-server.json": "release/llama-server.json",
  "resources/release/litestream.json": "release/litestream.json",
  "resources/release/source.json": "release/source.json",
  "resources/release/licenses/llama.cpp-LICENSE": "licenses/llama.cpp-LICENSE",
  "resources/release/licenses/litestream-LICENSE": "licenses/litestream-LICENSE",
  "resources/release/licenses/litestream-NOTICE": "licenses/litestream-NOTICE",
  "resources/embedding-indexes/jina-v1.json": "embedding-indexes/jina-v1.json",
  "resources/embedding-indexes/jina-v1-golden.json": "embedding-indexes/jina-v1-golden.json",
};
assertEqual(release.bundle.resources, expectedResources, "release resources");
const recoveryEntry = main.indexOf("run_recovery_cli_if_requested()");
const applicationEntry = main.indexOf("kosh_lib::run()");
assert(
  recoveryEntry >= 0 && applicationEntry > recoveryEntry,
  "packaged recovery command must run before Tauri application startup",
);

console.info(
  `Release contracts passed: Kosh ${packageJson.version}, universal macOS ${pin.target.minimumSystemVersion}+, ad-hoc signed with explicit empty entitlements and bounded capabilities.`,
);

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function assertRegularFile(path, label) {
  const metadata = lstatSync(path);
  assert(metadata.isFile(), `${label} is not a regular file`);
  assert(!metadata.isSymbolicLink(), `${label} must not be a symlink`);
}

function readPngHeader(path) {
  const bytes = readFileSync(path);
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  assert(bytes.subarray(0, signature.length).equals(signature), `${path} must be a PNG`);
  assert(bytes.toString("ascii", 12, 16) === "IHDR", `${path} must start with IHDR`);
  const colorType = bytes.readUInt8(25);
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
    bitDepth: bytes.readUInt8(24),
    hasAlpha: colorType === 4 || colorType === 6 || bytes.includes(Buffer.from("tRNS")),
  };
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
