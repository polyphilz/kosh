import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import {
  assert,
  assertEqual,
  DistributionSidecarKey,
  readDistributionSigningPolicy,
  run,
  signatureField,
  signatureFields,
} from "./distribution-signing.mjs";
import {
  DistributionArtifact,
  readSubmissionState,
  requireSignedSidecarSha256,
  sha256File,
} from "./distribution-notarization-state.mjs";

const policy = readDistributionSigningPolicy();
const scriptArguments = process.argv.slice(2);
if (scriptArguments[0] === "--") {
  scriptArguments.shift();
}
const appPath = resolve(
  scriptArguments[0] ?? "src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app",
);
const dmgPath = resolve(requiredArgument(1, "notarized DMG path"));
assert(existsSync(appPath), `application was not found: ${appPath}`);
assert(existsSync(dmgPath), `disk image was not found: ${dmgPath}`);

const suppliedApplicationSubmissionPath = scriptArguments[2];
const applicationSubmissionPath = suppliedApplicationSubmissionPath
  ? resolve(suppliedApplicationSubmissionPath)
  : undefined;
assert(
  applicationSubmissionPath === undefined || existsSync(applicationSubmissionPath),
  `application notarization state was not found: ${applicationSubmissionPath}`,
);
const expectedSidecarSha256 = applicationSubmissionPath
  ? requireSignedSidecarSha256(
      readSubmissionState(DistributionArtifact.Application, applicationSubmissionPath),
    )
  : signedSidecarSha256(appPath);
const expectedApplicationCodeDirectoryHash = verifyNotarizedApplication(appPath);

run("/usr/bin/codesign", ["--verify", "--strict", "--verbose=2", dmgPath]);
const dmgSignature = run("/usr/bin/codesign", ["--display", "--verbose=4", dmgPath]);
assertEqual(
  signatureField(dmgSignature, "TeamIdentifier"),
  policy.application.teamIdentifier,
  "DMG signing team",
);
assertEqual(
  signatureFields(dmgSignature, "Authority")[0],
  policy.application.signingIdentity,
  "DMG leaf signing authority",
);
run("xcrun", ["stapler", "validate", dmgPath]);
run("/usr/sbin/spctl", [
  "--assess",
  "--type",
  "open",
  "--context",
  "context:primary-signature",
  "--verbose=4",
  dmgPath,
]);

const mountRoot = mkdtempSync(join(tmpdir(), "kosh-distribution-mount-"));
const mountPoint = join(mountRoot, "volume");
mkdirSync(mountPoint);
let attached = false;
try {
  run("hdiutil", ["attach", dmgPath, "-readonly", "-nobrowse", "-mountpoint", mountPoint]);
  attached = true;
  const mountedAppPath = join(mountPoint, "Kosh.app");
  assert(existsSync(mountedAppPath), "notarized disk image does not contain Kosh.app");
  assertEqual(
    verifyNotarizedApplication(mountedAppPath),
    expectedApplicationCodeDirectoryHash,
    "mounted application code directory hash",
  );
} finally {
  if (attached) {
    run("hdiutil", ["detach", mountPoint]);
  }
  rmSync(mountRoot, { recursive: true, force: true });
}

console.info(`Notarization, stapling, Gatekeeper, and DMG contents passed.`);

function requiredArgument(index, label) {
  const value = scriptArguments[index];
  assert(value, `${label} is required`);
  return value;
}

function verifySignedSidecarHashes(applicationPath) {
  const actualSidecarSha256 = signedSidecarSha256(applicationPath);
  for (const key of Object.values(DistributionSidecarKey)) {
    assertEqual(
      actualSidecarSha256[key],
      expectedSidecarSha256[key],
      `${policy.sidecars[key].component} signed bytes from the submitted application`,
    );
  }
}

function verifyNotarizedApplication(applicationPath) {
  verifySignedSidecarHashes(applicationPath);
  run("xcrun", ["stapler", "validate", applicationPath]);
  run("/usr/sbin/spctl", ["--assess", "--type", "execute", "--verbose=4", applicationPath]);
  run("node", ["scripts/check-packaged-app.mjs", applicationPath, "developer-id"]);
  return signatureField(
    run("/usr/bin/codesign", ["--display", "--verbose=4", applicationPath]),
    "CDHash",
  );
}

function signedSidecarSha256(applicationPath) {
  const resourcesPath = join(applicationPath, "Contents/Resources");
  return Object.fromEntries(
    Object.values(DistributionSidecarKey).map((key) => [
      key,
      sha256File(join(resourcesPath, policy.sidecars[key].bundlePath)),
    ]),
  );
}
