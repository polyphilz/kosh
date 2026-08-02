import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

import {
  assert,
  assertEqual,
  DistributionSidecarKey,
  readDistributionSigningPolicy,
  run,
} from "./distribution-signing.mjs";
import {
  DistributionArtifact,
  readSubmissionState,
  requireSignedSidecarSha256,
  sha256File,
  writeSubmissionState,
} from "./distribution-notarization-state.mjs";
const NotarizationEnvironmentVariable = Object.freeze({
  Issuer: "APPLE_API_ISSUER",
  KeyId: "APPLE_API_KEY",
  KeyPath: "APPLE_API_KEY_PATH",
});
const NotarizationStatus = Object.freeze({
  Accepted: "Accepted",
  InProgress: "In Progress",
  Invalid: "Invalid",
  Rejected: "Rejected",
});
const ResumeArgument = "--resume";
const notarizationEnvironmentPath = resolve(".env.notarization");
const notarizationPollIntervalMilliseconds = 30_000;
const maximumConsecutivePollFailures = 6;

if (existsSync(notarizationEnvironmentPath)) {
  process.loadEnvFile(notarizationEnvironmentPath);
}

const arguments_ = process.argv.slice(2);
assert(
  arguments_.every((argument) => argument === ResumeArgument) && arguments_.length <= 1,
  `usage: node scripts/build-notarized-distribution.mjs [${ResumeArgument}]`,
);
const resume = arguments_.includes(ResumeArgument);
run("pnpm", ["release:verify-source"], { stdio: "inherit" });
const policy = readDistributionSigningPolicy();
const notarization = takeNotarizationEnvironment();
run("pnpm", ["release:verify-updater-signing"], { stdio: "inherit" });
preflight(notarization, policy);

const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const appPath = resolve("src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app");
const dmgPath = resolve(
  `src-tauri/target/universal-apple-darwin/release/bundle/dmg/Kosh_${packageJson.version}_universal.dmg`,
);
const notarizationDirectory = resolve(
  "src-tauri/target/universal-apple-darwin/release/bundle/notarization",
);
const appArchivePath = join(notarizationDirectory, "Kosh.zip");
const dmgUploadPath = join(notarizationDirectory, `Kosh_${packageJson.version}_universal.dmg`);
const appSubmissionPath = join(notarizationDirectory, "application.json");
const dmgSubmissionPath = join(notarizationDirectory, "disk-image.json");

if (resume) {
  assert(
    existsSync(appPath),
    `cannot resume because the signed application is missing: ${appPath}`,
  );
  assert(
    existsSync(appSubmissionPath),
    `cannot resume because application submission state is missing: ${appSubmissionPath}`,
  );
  console.info("Resuming the existing notarization submissions and artifacts.");
} else {
  rmSync(appSubmissionPath, { force: true });
  rmSync(dmgSubmissionPath, { force: true });
  buildSignedApplication(policy);
  assert(existsSync(appPath), `signed application was not created: ${appPath}`);
  mkdirSync(notarizationDirectory, { recursive: true });
  createApplicationArchive(appPath, appArchivePath);
  submitAndRecord(
    DistributionArtifact.Application,
    appArchivePath,
    appSubmissionPath,
    notarization,
    signedSidecarSha256(appPath, policy),
  );
}

ensureApplicationSidecarHashes(appSubmissionPath, appPath, policy);

completeNotarization(DistributionArtifact.Application, appSubmissionPath, appPath, notarization);

if (!resume || !existsSync(dmgSubmissionPath)) {
  createSignedDiskImage(appPath, dmgPath, policy);
  mkdirSync(notarizationDirectory, { recursive: true });
  copyFileSync(dmgPath, dmgUploadPath);
  submitAndRecord(DistributionArtifact.DiskImage, dmgUploadPath, dmgSubmissionPath, notarization);
} else {
  assert(existsSync(dmgPath), `cannot resume because the signed disk image is missing: ${dmgPath}`);
}

completeNotarization(DistributionArtifact.DiskImage, dmgSubmissionPath, dmgPath, notarization);
run("pnpm", ["release:verify-distribution", appPath, dmgPath, appSubmissionPath], {
  stdio: "inherit",
});

run("node", ["scripts/create-release-artifacts.mjs", appPath, dmgPath], {
  stdio: "inherit",
});

console.info(`Notarized distribution passed: ${dmgPath}`);

function buildSignedApplication(signingPolicy) {
  run("pnpm", ["release:verify-contracts"], { stdio: "inherit" });
  run("pnpm", ["release:stage-sidecars"], { stdio: "inherit" });
  run("pnpm", ["release:stage-provenance"], { stdio: "inherit" });
  run("pnpm", ["release:verify-resources"], { stdio: "inherit" });
  run("pnpm", ["release:sign-sidecars"], { stdio: "inherit" });

  const temporaryDirectory = mkdtempSync(join(tmpdir(), "kosh-distribution-config-"));
  const distributionConfigPath = join(temporaryDirectory, "tauri.distribution.conf.json");
  writeFileSync(
    distributionConfigPath,
    `${JSON.stringify(
      {
        app: {
          security: {
            capabilities: ["default", "quick-add", "main-updater"],
          },
        },
        bundle: {
          targets: ["app"],
          macOS: {
            signingIdentity: signingPolicy.application.signingIdentity,
            hardenedRuntime: true,
          },
        },
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );

  try {
    run(
      "pnpm",
      [
        "exec",
        "tauri",
        "build",
        "--config",
        "src-tauri/tauri.release.conf.json",
        "--config",
        distributionConfigPath,
        "--bundles",
        "app",
        "--target",
        "universal-apple-darwin",
      ],
      {
        env: {
          ...process.env,
          VITE_KOSH_UPDATER_ENABLED: "true",
        },
        stdio: "inherit",
      },
    );
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
  verifyPackagedSidecarsMatchStaging(appPath, signingPolicy);
}

function createApplicationArchive(applicationPath, archivePath) {
  rmSync(archivePath, { force: true });
  run("ditto", ["-c", "-k", "--sequesterRsrc", "--keepParent", applicationPath, archivePath], {
    stdio: "inherit",
  });
}

function createSignedDiskImage(applicationPath, diskImagePath, signingPolicy) {
  const stagingDirectory = mkdtempSync(join(tmpdir(), "kosh-dmg-stage-"));
  mkdirSync(dirname(diskImagePath), { recursive: true });
  rmSync(diskImagePath, { force: true });
  try {
    run("ditto", [applicationPath, join(stagingDirectory, "Kosh.app")]);
    symlinkSync("/Applications", join(stagingDirectory, "Applications"), "dir");
    run(
      "hdiutil",
      [
        "create",
        "-volname",
        "Kosh",
        "-srcfolder",
        stagingDirectory,
        "-format",
        "UDZO",
        diskImagePath,
      ],
      { stdio: "inherit" },
    );
    run(
      "/usr/bin/codesign",
      [
        "--force",
        "--timestamp",
        "--sign",
        signingPolicy.application.signingIdentity,
        diskImagePath,
      ],
      { stdio: "inherit" },
    );
  } finally {
    rmSync(stagingDirectory, { recursive: true, force: true });
  }
}

function submitAndRecord(artifact, uploadPath, statePath, credentials, signedSidecarHashes) {
  const result = runNotarytoolJson(["submit", uploadPath, "--output-format", "json"], credentials);
  assert(
    typeof result.id === "string" && result.id.length > 0,
    `Apple did not return a submission ID for ${artifact}`,
  );
  writeSubmissionState(statePath, {
    artifact,
    submissionId: result.id,
    uploadPath,
    signedSidecarSha256: signedSidecarHashes,
  });
  console.info(`Submitted ${artifact} to Apple as ${result.id}.`);
}

function completeNotarization(artifact, statePath, staplePath, credentials) {
  const state = readSubmissionState(artifact, statePath);
  waitForAcceptedSubmission(state.submissionId, artifact, credentials);
  retry(
    () => runNotarytool(["log", state.submissionId], credentials, { stdio: "inherit" }),
    `retrieving Apple's ${artifact} scan log`,
  );
  retry(
    () =>
      run("xcrun", ["stapler", "staple", staplePath], {
        stdio: "inherit",
      }),
    `stapling ${artifact}`,
  );
  run("xcrun", ["stapler", "validate", staplePath], { stdio: "inherit" });
}

function waitForAcceptedSubmission(submissionId, artifact, credentials) {
  let consecutiveFailures = 0;
  for (;;) {
    let result;
    try {
      result = runNotarytoolJson(["info", submissionId, "--output-format", "json"], credentials);
      consecutiveFailures = 0;
    } catch (error) {
      consecutiveFailures += 1;
      if (consecutiveFailures >= maximumConsecutivePollFailures) {
        throw new Error(
          `Apple status checks failed ${consecutiveFailures} consecutive times. ` +
            "The submission ID is saved; rerun pnpm release:resume:distribution.",
          { cause: error },
        );
      }
      console.warn(
        `Apple status check failed (${consecutiveFailures}/${maximumConsecutivePollFailures}); retrying in 30 seconds.`,
      );
      sleep(notarizationPollIntervalMilliseconds);
      continue;
    }

    if (result.status === NotarizationStatus.Accepted) {
      console.info(`Apple accepted ${artifact} submission ${submissionId}.`);
      return;
    }
    if (result.status === NotarizationStatus.InProgress) {
      console.info(
        `${artifact} submission ${submissionId} is still in progress; checking again in 30 seconds.`,
      );
      sleep(notarizationPollIntervalMilliseconds);
      continue;
    }
    if (
      result.status === NotarizationStatus.Invalid ||
      result.status === NotarizationStatus.Rejected
    ) {
      runNotarytool(["log", submissionId], credentials, { stdio: "inherit" });
      throw new Error(
        `Apple ${result.status.toLowerCase()} ${artifact} submission ${submissionId}`,
      );
    }
    throw new Error(
      `Unknown Apple notarization status for ${submissionId}: ${JSON.stringify(result.status)}`,
    );
  }
}

function retry(operation, label) {
  let lastError;
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      return operation();
    } catch (error) {
      lastError = error;
      if (attempt < 3) {
        console.warn(`${label} failed; retrying in 10 seconds.`);
        sleep(10_000);
      }
    }
  }
  throw new Error(`${label} failed. Rerun pnpm release:resume:distribution.`, { cause: lastError });
}

function runNotarytool(arguments_, credentials, options = {}) {
  return run(
    "xcrun",
    [
      "notarytool",
      ...arguments_,
      "--key",
      credentials.keyPath,
      "--key-id",
      credentials.keyId,
      "--issuer",
      credentials.issuer,
    ],
    options,
  );
}

function runNotarytoolJson(arguments_, credentials) {
  const result = spawnSync(
    "xcrun",
    [
      "notarytool",
      ...arguments_,
      "--key",
      credentials.keyPath,
      "--key-id",
      credentials.keyId,
      "--issuer",
      credentials.issuer,
    ],
    {
      encoding: "utf8",
      stdio: "pipe",
    },
  );
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
    throw new Error(`xcrun notarytool failed with exit code ${result.status ?? "unknown"}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw new Error("Apple returned invalid JSON from notarytool", {
      cause: error,
    });
  }
}

function takeNotarizationEnvironment() {
  const values = Object.fromEntries(
    Object.values(NotarizationEnvironmentVariable).map((name) => [name, process.env[name]?.trim()]),
  );
  for (const name of Object.values(NotarizationEnvironmentVariable)) {
    delete process.env[name];
  }
  const value = (name) => {
    const result = values[name];
    assert(result, `${name} is required; copy .env.notarization.example to .env.notarization`);
    return result;
  };
  return {
    issuer: value(NotarizationEnvironmentVariable.Issuer),
    keyId: value(NotarizationEnvironmentVariable.KeyId),
    keyPath: resolve(value(NotarizationEnvironmentVariable.KeyPath)),
  };
}

function preflight(notarization, signingPolicy) {
  assert(process.platform === "darwin", "distribution builds require macOS");
  assert(
    existsSync(notarization.keyPath),
    `notarization key was not found: ${notarization.keyPath}`,
  );
  const keyMetadata = statSync(notarization.keyPath);
  assert(keyMetadata.isFile(), "notarization key is not a regular file");
  if ((keyMetadata.mode & 0o077) !== 0) {
    chmodSync(notarization.keyPath, 0o600);
  }
  const identities = run("security", ["find-identity", "-v", "-p", "codesigning"]);
  assert(
    identities.includes(signingPolicy.application.signingIdentity),
    `Keychain does not contain ${signingPolicy.application.signingIdentity}`,
  );
  retry(() => runNotarytool(["history"], notarization), "validating Apple credentials");
  console.info(`Apple credentials passed for ${basename(notarization.keyPath)}.`);
}

function ensureApplicationSidecarHashes(statePath, applicationPath, signingPolicy) {
  const state = readSubmissionState(DistributionArtifact.Application, statePath);
  if (state.signedSidecarSha256 !== undefined) {
    requireSignedSidecarSha256(state);
    return;
  }

  const extractionDirectory = mkdtempSync(join(tmpdir(), "kosh-notarization-archive-"));
  try {
    run("ditto", ["-x", "-k", state.uploadPath, extractionDirectory]);
    const archivedApplicationPath = join(extractionDirectory, basename(applicationPath));
    const archivedHashes = signedSidecarSha256(archivedApplicationPath, signingPolicy);
    assertEqual(
      signedSidecarSha256(applicationPath, signingPolicy),
      archivedHashes,
      "current application sidecars from the submitted archive",
    );
    writeSubmissionState(statePath, {
      artifact: state.artifact,
      submissionId: state.submissionId,
      uploadPath: state.uploadPath,
      signedSidecarSha256: archivedHashes,
    });
  } finally {
    rmSync(extractionDirectory, { recursive: true, force: true });
  }
}

function verifyPackagedSidecarsMatchStaging(applicationPath, signingPolicy) {
  const packagedHashes = signedSidecarSha256(applicationPath, signingPolicy);
  for (const key of Object.values(DistributionSidecarKey)) {
    assertEqual(
      packagedHashes[key],
      sha256File(resolve(signingPolicy.sidecars[key].stagingPath)),
      `packaged ${signingPolicy.sidecars[key].component} signed bytes`,
    );
  }
}

function signedSidecarSha256(applicationPath, signingPolicy) {
  const resourcesPath = resolve(applicationPath, "Contents/Resources");
  return {
    [DistributionSidecarKey.LlamaServer]: sha256File(
      resolve(resourcesPath, signingPolicy.sidecars[DistributionSidecarKey.LlamaServer].bundlePath),
    ),
    [DistributionSidecarKey.Litestream]: sha256File(
      resolve(resourcesPath, signingPolicy.sidecars[DistributionSidecarKey.Litestream].bundlePath),
    ),
  };
}

function sleep(milliseconds) {
  Atomics.wait(
    new Int32Array(new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT)),
    0,
    0,
    milliseconds,
  );
}
