import { readFileSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";

const pinPath = "src-tauri/resources/sidecars/litestream-v1.json";
const noticePath = "src-tauri/resources/sidecars/litestream-NOTICE";
const releaseConfigPath = "src-tauri/tauri.release.conf.json";
const packagePath = "package.json";
const rustContractPath = "src-tauri/src/backup/litestream.rs";
const localProtocolPath = "../scripts/verify-litestream-local-protocol.sh";
const r2ProtocolPath = "../scripts/verify-litestream-r2-protocol.sh";
const ciProtocolPath = "../scripts/verify-litestream-ci-protocol.sh";
const stagePath = "../scripts/stage-litestream-sidecar.sh";
const envExamplePath = ".env.example";

const pin = readJson(pinPath);
const releaseConfig = readJson(releaseConfigPath);
const packageJson = readJson(packagePath);
const rustContract = readFileSync(rustContractPath, "utf8");
const localProtocol = readFileSync(localProtocolPath, "utf8");
const r2Protocol = readFileSync(r2ProtocolPath, "utf8");
const ciProtocol = readFileSync(ciProtocolPath, "utf8");
const stage = readFileSync(stagePath, "utf8");
const notice = readFileSync(noticePath, "utf8");
const envExample = readFileSync(envExamplePath, "utf8");

assertEqual(pin.manifestVersion, 1, "Litestream pin format");
assertEqual(pin.component, "litestream", "Litestream component");
assertEqual(pin.upstream.releaseTag, "v0.5.15", "Litestream release");
assertEqual(pin.target.operatingSystem, "macos", "Litestream target OS");
assertEqual(pin.target.architectures, ["arm64", "x86_64"], "Litestream architectures");
assertEqual(pin.target.minimumSystemVersion, "14.0", "packaged minimum macOS");
assertEqual(pin.binary.bundlePath, "bin/litestream", "Litestream bundle path");
assertEqual(
  pin.binary.trustedCleanupSha256s,
  ["c535829126d7bb8f3e8c2e7a4f9e3507c63dad1ed91815824aeabf9a5217760b"],
  "append-only trusted Litestream cleanup pins",
);
assertEqual(
  pin.binary.codeSignatureIdentifier,
  undefined,
  "signature identifier must be scoped to the universal pin",
);
assertEqual(
  pin.binary.universal.codeSignatureIdentifier,
  "com.rohan.kosh.litestream",
  "Litestream signature identifier",
);
assertEqual(pin.binary.universal.size, 77_508_256, "universal byte length");
assertSha256(pin.binary.universal.sha256, "universal Litestream");
assertEqual(pin.verification.requiredL0Retention, "720h", "exact-TXID retention");
for (const [name, value] of Object.entries(pin.verification)) {
  if (typeof value === "boolean") {
    assert(value, `Litestream protocol verification is not pinned green: ${name}`);
  }
}
assertSha256(pin.upstream.checksums.sha256, "official checksums");
assert(pin.upstream.checksums.size > 0, "invalid official checksums size");
for (const architecture of pin.target.architectures) {
  const asset = pin.upstream.assets[architecture];
  assert(asset, `missing ${architecture} Litestream asset`);
  assert(
    asset.url.includes(`/download/${pin.upstream.releaseTag}/${asset.name}`),
    `${architecture} asset URL is not release-pinned`,
  );
  assert(asset.size > 0, `invalid ${architecture} archive size`);
  assert(asset.binarySize > 0, `invalid ${architecture} binary size`);
  assertSha256(asset.sha256, `${architecture} archive`);
  assertSha256(asset.binarySha256, `${architecture} binary`);
  assertEqual(
    pin.binary.versionOutputByArchitecture[architecture],
    "0.5.15",
    `${architecture} version output`,
  );
  assertEqual(
    pin.target.binaryMinimumSystemVersionByArchitecture[architecture],
    "12.0",
    `${architecture} deployment target`,
  );
}

const resourceMappings = releaseConfig.bundle.resources;
const expectedMappings = {
  [`resources/release/${pin.stagingPaths.binary}`]: pin.resourceDestinations.binary,
  [`resources/release/${pin.stagingPaths.releaseManifest}`]:
    pin.resourceDestinations.releaseManifest,
  [`resources/release/${pin.stagingPaths.license}`]: pin.resourceDestinations.license,
  [`resources/release/${pin.stagingPaths.notice}`]: pin.resourceDestinations.notice,
};
for (const [source, destination] of Object.entries(expectedMappings)) {
  assertEqual(resourceMappings[source], destination, `Tauri resource ${source}`);
}
assertEqual(
  releaseConfig.bundle.macOS.minimumSystemVersion,
  pin.target.minimumSystemVersion,
  "packaged minimum macOS version",
);

assertEqual(
  packageJson.scripts["release:stage-litestream"],
  "../scripts/stage-litestream-sidecar.sh",
  "Litestream staging command",
);
assert(
  packageJson.scripts["release:stage-sidecars"].includes("release:stage-litestream"),
  "release staging does not include Litestream",
);
for (const command of [
  "release:verify-source",
  "release:verify-contracts",
  "release:stage-sidecars",
  "release:verify-resources",
  "tauri build",
  "release:verify-app",
]) {
  assert(
    packageJson.scripts["release:build:app"].includes(command),
    `release build omits ${command}`,
  );
}
assertEqual(
  packageJson.scripts["litestream:verify-local"],
  "../scripts/verify-litestream-local-protocol.sh",
  "local protocol command",
);
assertEqual(
  packageJson.scripts["litestream:verify-r2"],
  "../scripts/verify-litestream-r2-protocol.sh",
  "R2 protocol command",
);

for (const contract of [
  'include_str!("../../resources/sidecars/litestream-v1.json")',
  "l0-retention: 720h",
  "auto-recover: false",
  "verify-compaction: true",
  "KOSH_LITESTREAM_R2_ACCESS_KEY_ID",
  "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY",
  "MAX_CONTROL_OUTPUT_BYTES",
  "ControlSocketPathTooLong",
]) {
  assert(rustContract.includes(contract), `Rust Litestream contract omits ${contract}`);
}
for (const contract of [
  "exactFenceRestore",
  "postCompactionExactRestore",
  "defaultL0ExpiryInteriorTxidFailureObserved",
  "gracefulShutdownFinalSync",
  "orphanProcess",
]) {
  assert(localProtocol.includes(contract), `local protocol omits ${contract}`);
}
for (const contract of [
  "KOSH_LITESTREAM_R2_JURISDICTION",
  "r2.cloudflarestorage.com",
  "remoteResidueObjects",
  "exactFenceRestore",
  "postCompactionExactRestore",
  "remote test prefix removed",
]) {
  assert(r2Protocol.includes(contract), `R2 protocol omits ${contract}`);
}
for (const contract of [
  "upstream.checksums",
  "upstream.assets[$architecture].binarySha256",
  "verify-litestream-local-protocol.sh",
]) {
  assert(ciProtocol.includes(contract), `CI protocol omits ${contract}`);
}
for (const contract of [
  "official checksums",
  "lipo -create",
  "codesign --force --sign -",
  "binary.universal.sha256",
]) {
  assert(stage.includes(contract), `Litestream staging omits ${contract}`);
}

assert(notice.includes("Apache License 2.0"), "Litestream notice omits its license");
assert(notice.includes("universal executable"), "Litestream notice omits universal assembly");
for (const path of [
  pinPath,
  noticePath,
  localProtocolPath,
  r2ProtocolPath,
  ciProtocolPath,
  stagePath,
]) {
  assert(statSync(path).isFile(), `${path} is not a regular file`);
}

const envLines = Object.fromEntries(
  envExample
    .split(/\r?\n/u)
    .filter((line) => line && !line.startsWith("#"))
    .map((line) => {
      const separator = line.indexOf("=");
      assert(separator > 0, "malformed .env.example line");
      return [line.slice(0, separator), line.slice(separator + 1)];
    }),
);
assertEqual(
  Object.keys(envLines).sort(),
  [
    "KOSH_LITESTREAM_R2_ACCESS_KEY_ID",
    "KOSH_LITESTREAM_R2_ACCOUNT_ID",
    "KOSH_LITESTREAM_R2_BUCKET",
    "KOSH_LITESTREAM_R2_JURISDICTION",
    "KOSH_LITESTREAM_R2_PREFIX",
    "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY",
  ].sort(),
  "Litestream environment template",
);
for (const secret of [
  "KOSH_LITESTREAM_R2_ACCOUNT_ID",
  "KOSH_LITESTREAM_R2_BUCKET",
  "KOSH_LITESTREAM_R2_ACCESS_KEY_ID",
  "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY",
]) {
  assertEqual(envLines[secret], "", `${secret} placeholder`);
}
assertEqual(envLines.KOSH_LITESTREAM_R2_JURISDICTION, "DEFAULT", "R2 jurisdiction default");
assertEqual(
  envLines.KOSH_LITESTREAM_R2_PREFIX,
  "kosh/primary/protocol-spike",
  "confined development prefix",
);

const tracked = execFileSync("git", ["ls-files", "--full-name", "-z"], {
  encoding: "utf8",
})
  .split("\0")
  .filter(Boolean);
for (const path of tracked) {
  const normalized = path.toLowerCase();
  assert(
    !normalized.startsWith("app/src-tauri/resources/release/"),
    `generated release staging is tracked: ${path}`,
  );
  assert(
    !normalized.includes("litestream.yml") && !normalized.includes("litestream.yaml"),
    `generated Litestream configuration is tracked: ${path}`,
  );
}

console.info(
  `Litestream release contracts passed: ${pin.upstream.releaseTag}, universal ${pin.target.architectures.join("+")}, ${pin.binary.universal.sha256}.`,
);

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function assertSha256(value, label) {
  assert(/^[a-f0-9]{64}$/u.test(value), `invalid ${label} SHA-256`);
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
