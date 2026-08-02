import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

export const DistributionSidecarKey = Object.freeze({
  LlamaServer: "llamaServer",
  Litestream: "litestream",
});

export const distributionSigningPolicyPath = resolve("src-tauri/distribution-signing.json");

export function readDistributionSigningPolicy() {
  const policy = JSON.parse(readFileSync(distributionSigningPolicyPath, "utf8"));
  assertEqual(policy.formatVersion, 1, "distribution signing format");
  assert(
    typeof policy.application?.bundleIdentifier === "string" &&
      policy.application.bundleIdentifier.length > 0,
    "distribution application bundle identifier is missing",
  );
  assert(
    typeof policy.application?.signingIdentity === "string" &&
      policy.application.signingIdentity.startsWith("Developer ID Application: "),
    "distribution signing identity is not a Developer ID Application identity",
  );
  assert(
    /^[A-Z0-9]{10}$/u.test(policy.application?.teamIdentifier),
    "distribution team identifier is invalid",
  );
  assertEqual(
    Object.keys(policy.sidecars).sort(),
    Object.values(DistributionSidecarKey).sort(),
    "distribution sidecar keys",
  );
  for (const sidecar of Object.values(policy.sidecars)) {
    for (const field of [
      "component",
      "identifier",
      "stagingPath",
      "bundlePath",
      "pinManifestPath",
    ]) {
      assert(
        typeof sidecar[field] === "string" && sidecar[field].length > 0,
        `distribution sidecar ${sidecar.component ?? "<unknown>"} is missing ${field}`,
      );
    }
  }
  return policy;
}

export function codeSigningRequirement(policy, identifier) {
  return [
    `identifier ${quoteRequirementString(identifier)}`,
    "anchor apple generic",
    `certificate leaf[subject.OU] = ${quoteRequirementString(policy.application.teamIdentifier)}`,
    `certificate leaf[subject.CN] = ${quoteRequirementString(policy.application.signingIdentity)}`,
  ].join(" and ");
}

export function verifyDeveloperIdSignature(path, policy, identifier) {
  run("/usr/bin/codesign", [
    "--verify",
    "--strict",
    "--verbose=2",
    `-R=${codeSigningRequirement(policy, identifier)}`,
    path,
  ]);
  const signature = run("/usr/bin/codesign", ["--display", "--verbose=4", path]);
  assertEqual(signatureField(signature, "Identifier"), identifier, `${path} signing identifier`);
  assertEqual(
    signatureField(signature, "TeamIdentifier"),
    policy.application.teamIdentifier,
    `${path} signing team`,
  );
  assertEqual(
    signatureFields(signature, "Authority")[0],
    policy.application.signingIdentity,
    `${path} leaf signing authority`,
  );
  assert(
    signature.includes("flags=0x10000(runtime)"),
    `${path} is not signed with the hardened runtime`,
  );
}

export function signatureField(signature, field) {
  const values = signatureFields(signature, field);
  assertEqual(values.length, 1, `${field} signature field count`);
  return values[0];
}

export function signatureFields(signature, field) {
  const prefix = `${field}=`;
  return signature
    .split(/\r?\n/u)
    .filter((line) => line.startsWith(prefix))
    .map((line) => line.slice(prefix.length));
}

export function run(command, arguments_, options = {}) {
  const { capture = false, ...spawnOptions } = options;
  const result = spawnSync(command, arguments_, {
    encoding: "utf8",
    stdio: capture ? "pipe" : undefined,
    ...spawnOptions,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    if (result.stdout) {
      process.stderr.write(result.stdout);
    }
    if (result.stderr) {
      process.stderr.write(result.stderr);
    }
    throw new Error(`${command} failed with exit code ${result.status ?? "unknown"}`);
  }
  return capture
    ? `${result.stdout ?? ""}`.trim()
    : `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
}

export function assertEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Unexpected ${label}: ${JSON.stringify(actual)}; expected ${JSON.stringify(expected)}`,
    );
  }
}

export function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function quoteRequirementString(value) {
  return JSON.stringify(value);
}
