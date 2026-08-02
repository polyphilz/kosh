import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { assert, run } from "./distribution-signing.mjs";

const provenancePath = resolve("src-tauri/resources/release/source.json");
const commit = run("git", ["rev-parse", "HEAD"], { capture: true });

assert(/^[a-f0-9]{40}$/u.test(commit), `invalid source commit: ${commit}`);

mkdirSync(dirname(provenancePath), { recursive: true });
writeFileSync(
  provenancePath,
  `${JSON.stringify(
    {
      commit,
      dirty: run("git", ["status", "--porcelain"], { capture: true }).length > 0,
    },
    null,
    2,
  )}\n`,
);

console.info(`Release provenance staged: ${provenancePath}`);
