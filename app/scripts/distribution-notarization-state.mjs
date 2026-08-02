import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";

import { assert, assertEqual, DistributionSidecarKey } from "./distribution-signing.mjs";

export const DistributionArtifact = Object.freeze({
  Application: "application",
  DiskImage: "disk-image",
});

const SubmissionStateFormatVersion = Object.freeze({
  Legacy: 1,
  SignedSidecars: 2,
});

export function readSubmissionState(expectedArtifact, statePath) {
  const state = JSON.parse(readFileSync(statePath, "utf8"));
  assert(
    Object.values(SubmissionStateFormatVersion).includes(state.formatVersion),
    "unsupported notarization state format",
  );
  assert(state.artifact === expectedArtifact, `unexpected notarization artifact in ${statePath}`);
  assert(
    typeof state.submissionId === "string" && state.submissionId.length > 0,
    `missing Apple submission ID in ${statePath}`,
  );
  assert(
    typeof state.uploadPath === "string" && existsSync(state.uploadPath),
    `notarization upload is missing: ${state.uploadPath}`,
  );
  assert(
    sha256File(state.uploadPath) === state.uploadSha256,
    `notarization upload changed after submission: ${state.uploadPath}`,
  );
  return state;
}

export function writeSubmissionState(
  statePath,
  { artifact, submissionId, uploadPath, signedSidecarSha256 },
) {
  const state = {
    formatVersion: SubmissionStateFormatVersion.SignedSidecars,
    artifact,
    submissionId,
    uploadPath,
    uploadSha256: sha256File(uploadPath),
  };
  if (signedSidecarSha256 !== undefined) {
    state.signedSidecarSha256 = validateSignedSidecarSha256(signedSidecarSha256);
  }
  writeFileSync(statePath, `${JSON.stringify(state, null, 2)}\n`, {
    mode: 0o600,
  });
  return state;
}

export function requireSignedSidecarSha256(state) {
  assert(
    state.signedSidecarSha256 !== undefined,
    "application notarization state is missing signed sidecar hashes",
  );
  return validateSignedSidecarSha256(state.signedSidecarSha256);
}

export function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function validateSignedSidecarSha256(value) {
  assert(
    value !== null && typeof value === "object" && !Array.isArray(value),
    "signed sidecar hashes must be an object",
  );
  assertEqual(
    Object.keys(value).sort(),
    Object.values(DistributionSidecarKey).sort(),
    "signed sidecar hash keys",
  );
  for (const [key, hash] of Object.entries(value)) {
    assert(
      typeof hash === "string" && /^[a-f0-9]{64}$/u.test(hash),
      `signed sidecar hash is invalid for ${key}`,
    );
  }
  return value;
}
