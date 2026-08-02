import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  assert,
  assertEqual,
  readDistributionSigningPolicy,
  run,
  verifyDeveloperIdSignature,
} from "./distribution-signing.mjs";

const policy = readDistributionSigningPolicy();

for (const sidecar of Object.values(policy.sidecars)) {
  const path = resolve(sidecar.stagingPath);
  const manifest = JSON.parse(readFileSync(resolve(sidecar.pinManifestPath), "utf8"));
  const binary = manifest.binary.universal ?? manifest.binary;
  const metadata = lstatSync(path);
  assert(metadata.isFile(), `${sidecar.component} is not a regular file`);
  assert(!metadata.isSymbolicLink(), `${sidecar.component} must not be a symbolic link`);
  assert((metadata.mode & 0o111) !== 0, `${sidecar.component} is not executable`);
  assertEqual(metadata.size, binary.size, `${sidecar.component} unsigned byte length`);
  assertEqual(sha256File(path), binary.sha256, `${sidecar.component} unsigned SHA-256`);

  run("/usr/bin/codesign", [
    "--force",
    "--sign",
    policy.application.signingIdentity,
    "--identifier",
    sidecar.identifier,
    "--options",
    "runtime",
    "--timestamp",
    path,
  ]);
  verifyDeveloperIdSignature(path, policy, sidecar.identifier);
  console.info(`Signed ${sidecar.component} as ${sidecar.identifier}.`);
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}
