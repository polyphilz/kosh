import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import assert from "node:assert/strict";

import { selectMigrationSnapshot } from "./select-migration-snapshot.mjs";

const root = mkdtempSync(join(tmpdir(), "kosh-release-snapshot-test-"));

try {
  for (const name of [
    "migration-100-reviewed",
    "migration-200-invalid",
    "migration-300-newer",
    "unrelated-400",
  ]) {
    mkdirSync(join(root, name));
  }

  const inspected = [];
  const selected = selectMigrationSnapshot({
    root,
    expectedMainHead: 16,
    expectedMediaHead: 2,
    inspect(snapshot) {
      const name = basename(snapshot.directory);
      inspected.push(name);
      if (name === "migration-200-invalid") throw new Error("corrupt snapshot");
      if (name === "migration-300-newer") return { main: 17, media: 2 };
      return { main: 16, media: 2 };
    },
  });

  assert.equal(basename(selected.directory), "migration-100-reviewed");
  assert.deepEqual(inspected, [
    "migration-300-newer",
    "migration-200-invalid",
    "migration-100-reviewed",
  ]);
  assert.throws(
    () =>
      selectMigrationSnapshot({
        root,
        expectedMainHead: 15,
        expectedMediaHead: 1,
        inspect: () => ({ main: 16, media: 2 }),
      }),
    /no verified migration snapshot matches main V15 and media V1 across 3 candidate\(s\)/u,
  );
  assert.throws(
    () =>
      selectMigrationSnapshot({
        root,
        expectedMainHead: 0,
        expectedMediaHead: 2,
        inspect: () => ({ main: 16, media: 2 }),
      }),
    /main migration head must be a positive safe integer/u,
  );

  console.info("release migration snapshot selection tests passed");
} finally {
  rmSync(root, { recursive: true });
}
