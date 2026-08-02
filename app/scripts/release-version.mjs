import { readFileSync } from "node:fs";

import { assert, assertEqual } from "./distribution-signing.mjs";

const SEMANTIC_VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u;

export function readReleaseVersion() {
  const packageVersion = readJson("package.json").version;
  const tauriVersion = readJson("src-tauri/tauri.conf.json").version;
  const cargoManifest = readFileSync("src-tauri/Cargo.toml", "utf8");
  const cargoVersion = cargoManifest.match(/^version\s*=\s*"([^"]+)"/mu)?.[1];

  assert(
    typeof packageVersion === "string" && SEMANTIC_VERSION.test(packageVersion),
    `package.json has an invalid release version: ${packageVersion}`,
  );
  assertEqual(tauriVersion, packageVersion, "Tauri release version");
  assertEqual(cargoVersion, packageVersion, "Cargo release version");
  return packageVersion;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}
