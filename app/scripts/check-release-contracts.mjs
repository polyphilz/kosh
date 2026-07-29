import { lstatSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const local = readJson("src-tauri/tauri.conf.json");
const release = readJson("src-tauri/tauri.release.conf.json");
const packageJson = readJson("package.json");
const pin = readJson("src-tauri/resources/sidecars/llama-server-v1.json");
const cargo = readFileSync("src-tauri/Cargo.toml", "utf8");

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
  run("sips", [
    "-g",
    "pixelWidth",
    "-g",
    "pixelHeight",
    "-g",
    "hasAlpha",
    "src-tauri/icons/tray-icon.png",
  ])
    .split(/\r?\n/u)
    .slice(1)
    .map((line) => line.trim()),
  ["pixelWidth: 32", "pixelHeight: 32", "hasAlpha: yes"],
  "tray template dimensions",
);

const entitlements = JSON.parse(
  run("plutil", ["-convert", "json", "-o", "-", "src-tauri/Entitlements.plist"]),
);
assertEqual(entitlements, {}, "least-privilege entitlements");

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

const expectedResources = {
  "resources/release/bin/llama-server": "bin/llama-server",
  "resources/release/llama-server.json": "release/llama-server.json",
  "resources/release/licenses/llama.cpp-LICENSE": "licenses/llama.cpp-LICENSE",
  "resources/embedding-indexes/jina-v1.json": "embedding-indexes/jina-v1.json",
  "resources/embedding-indexes/jina-v1-golden.json": "embedding-indexes/jina-v1-golden.json",
};
assertEqual(release.bundle.resources, expectedResources, "release resources");

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
