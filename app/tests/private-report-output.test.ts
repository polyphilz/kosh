import { mkdtemp, mkdir, readFile, realpath, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, test } from "vitest";
// @ts-expect-error The production helper is intentionally plain Node ESM.
import { writePrivateReport } from "../scripts/private-report-output.mjs";

const roots: string[] = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

test("publishes and replaces a private regular report", async () => {
  const parent = await temporaryRoot();
  const root = join(parent, "reports");
  const output = join(root, "performance.json");
  await writePrivateReport(root, output, "first\n");
  await writePrivateReport(root, output, "second\n");
  await expect(readFile(output, "utf8")).resolves.toBe("second\n");
});

test("rejects a symlinked output without changing its target", async () => {
  const parent = await temporaryRoot();
  const root = join(parent, "reports");
  const target = join(parent, "target.txt");
  await mkdir(root);
  await writePrivateReport(parent, target, "untouched\n");
  await symlink(target, join(root, "performance.json"));

  await expect(
    writePrivateReport(root, join(root, "performance.json"), "overwrite\n"),
  ).rejects.toThrow("absent or a regular file");
  await expect(readFile(target, "utf8")).resolves.toBe("untouched\n");
});

test("rejects a report root reached through a symlink", async () => {
  const parent = await temporaryRoot();
  const actual = join(parent, "actual");
  const linked = join(parent, "linked");
  await mkdir(actual);
  await symlink(actual, linked);

  await expect(
    writePrivateReport(linked, join(linked, "performance.json"), "nope\n"),
  ).rejects.toThrow(/real directory|symlinked path components/u);
});

async function temporaryRoot() {
  const root = await realpath(await mkdtemp(join(tmpdir(), "kosh-private-report-")));
  roots.push(root);
  return root;
}
