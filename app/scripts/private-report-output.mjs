import { randomUUID } from "node:crypto";
import { lstat, mkdir, realpath, rename, unlink, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";

export async function writePrivateReport(rootPath, outputPath, contents) {
  const root = resolve(rootPath);
  const output = resolve(outputPath);
  if (dirname(output) !== root) {
    throw new Error(`report output must be a direct child of ${root}`);
  }

  await mkdir(root, { mode: 0o700, recursive: true });
  const rootMetadata = await lstat(root);
  if (rootMetadata.isSymbolicLink() || !rootMetadata.isDirectory()) {
    throw new Error(`report root must be a real directory: ${root}`);
  }
  if ((await realpath(root)) !== root) {
    throw new Error(`report root must not contain symlinked path components: ${root}`);
  }

  const outputMetadata = await metadataIfPresent(output);
  if (outputMetadata && (outputMetadata.isSymbolicLink() || !outputMetadata.isFile())) {
    throw new Error(`report output must be absent or a regular file: ${output}`);
  }

  const temporary = join(root, `.${basename(output)}.${process.pid}.${randomUUID()}.tmp`);
  try {
    await writeFile(temporary, contents, { encoding: "utf8", flag: "wx", mode: 0o600 });
    await rename(temporary, output);
  } catch (error) {
    await unlink(temporary).catch((cleanupError) => {
      if (cleanupError?.code !== "ENOENT") throw cleanupError;
    });
    throw error;
  }
}

async function metadataIfPresent(path) {
  try {
    return await lstat(path);
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}
