import { readdirSync } from "node:fs";
import { join } from "node:path";

export function selectMigrationSnapshot({ root, expectedMainHead, expectedMediaHead, inspect }) {
  assertMigrationHead(expectedMainHead, "main");
  assertMigrationHead(expectedMediaHead, "media");
  if (typeof inspect !== "function") {
    throw new TypeError("migration snapshot selection requires an inspector");
  }

  const candidates = readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.startsWith("migration-"))
    .map((entry) => snapshotPaths(join(root, entry.name)))
    .sort((left, right) => right.directory.localeCompare(left.directory));
  if (candidates.length === 0) {
    throw new Error("packaged upgrade created no migration snapshot");
  }

  for (const candidate of candidates) {
    try {
      const heads = inspect(candidate);
      if (heads.main === expectedMainHead && heads.media === expectedMediaHead) {
        return candidate;
      }
    } catch {
      // A later, unrelated invalid recovery point must not hide an earlier
      // verified snapshot for the exact reviewed migration fixture.
    }
  }

  throw new Error(
    `no verified migration snapshot matches main V${expectedMainHead} and media V${expectedMediaHead} across ${candidates.length} candidate(s)`,
  );
}

function snapshotPaths(directory) {
  return {
    directory,
    manifest: join(directory, "manifest.json"),
    main: join(directory, "kosh.sqlite3"),
    media: join(directory, "media.sqlite3"),
  };
}

function assertMigrationHead(value, label) {
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new TypeError(`${label} migration head must be a positive safe integer`);
  }
}
