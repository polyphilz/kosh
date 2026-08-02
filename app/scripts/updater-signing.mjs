import {
  chmodSync,
  existsSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve, sep } from "node:path";

import { assert, run } from "./distribution-signing.mjs";

export const UpdaterEnvironmentVariable = Object.freeze({
  PrivateKeyPassword: "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
  PrivateKeyPath: "TAURI_SIGNING_PRIVATE_KEY_PATH",
});

const updaterEnvironmentPath = resolve(".env.updater");
const repositoryRoot = resolve("..");
const tauriConfigPath = resolve("src-tauri/tauri.conf.json");

export function takeUpdaterEnvironment() {
  if (existsSync(updaterEnvironmentPath)) {
    process.loadEnvFile(updaterEnvironmentPath);
  }
  const values = Object.fromEntries(
    Object.values(UpdaterEnvironmentVariable).map((name) => [name, process.env[name]?.trim()]),
  );
  for (const name of Object.values(UpdaterEnvironmentVariable)) {
    delete process.env[name];
  }
  const value = (name) => {
    const result = values[name];
    assert(result, `${name} is required; copy .env.updater.example to .env.updater`);
    return result;
  };
  return {
    privateKeyPassword: value(UpdaterEnvironmentVariable.PrivateKeyPassword),
    privateKeyPath: resolve(value(UpdaterEnvironmentVariable.PrivateKeyPath)),
  };
}

export function preflightUpdaterCredentials(credentials) {
  assert(
    existsSync(credentials.privateKeyPath),
    `updater private key is missing: ${credentials.privateKeyPath}`,
  );
  assert(
    statSync(credentials.privateKeyPath).isFile(),
    "updater private key is not a regular file",
  );
  const privateKeyPath = realpathSync(credentials.privateKeyPath);
  const relativeKeyPath = relative(realpathSync(repositoryRoot), privateKeyPath);
  assert(
    relativeKeyPath === ".." || relativeKeyPath.startsWith(`..${sep}`),
    "updater private key must be stored outside the Kosh repository",
  );
  if ((statSync(privateKeyPath).mode & 0o077) !== 0) {
    chmodSync(privateKeyPath, 0o600);
  }
}

export function signUpdaterArchive(archivePath, credentials, { quiet = false } = {}) {
  run(
    "pnpm",
    [
      "exec",
      "tauri",
      "signer",
      "sign",
      "--private-key-path",
      credentials.privateKeyPath,
      archivePath,
    ],
    {
      env: {
        ...process.env,
        [UpdaterEnvironmentVariable.PrivateKeyPassword]: credentials.privateKeyPassword,
      },
      stdio: quiet ? "pipe" : "inherit",
    },
  );
  assert(existsSync(`${archivePath}.sig`), `updater signature was not created: ${archivePath}.sig`);
}

export function verifyUpdaterArchiveSignature(
  archivePath,
  signaturePath,
  configPath = tauriConfigPath,
) {
  run(
    "cargo",
    [
      "run",
      "--quiet",
      "--release",
      "--manifest-path",
      "scripts/updater-signature-verifier/Cargo.toml",
      "--",
      archivePath,
      signaturePath,
      configPath,
    ],
    { stdio: "inherit" },
  );
}

export function verifyUpdaterSigningCredentials(credentials) {
  preflightUpdaterCredentials(credentials);
  const temporaryDirectory = mkdtempSync(join(tmpdir(), "kosh-updater-signing-preflight-"));
  const probePath = join(temporaryDirectory, "probe.txt");
  try {
    writeFileSync(probePath, "Kosh updater signing preflight\n", {
      mode: 0o600,
    });
    signUpdaterArchive(probePath, credentials, { quiet: true });
    verifyUpdaterArchiveSignature(probePath, `${probePath}.sig`);
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}
