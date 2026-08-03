import { spawn } from "node:child_process";
import { platform } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const helper = resolve(dirname(fileURLToPath(import.meta.url)), "private-report-writer.py");

export async function writePrivateReport(rootPath, outputPath, contents, hooks = {}) {
  const root = resolve(rootPath);
  const output = resolve(outputPath);
  if (dirname(output) !== root) {
    throw new Error(`report output must be a direct child of ${root}`);
  }

  const command = platform() === "darwin" ? "/usr/bin/xcrun" : "python3";
  const arguments_ =
    platform() === "darwin" ? ["python3", helper, root, output] : [helper, root, output];
  const child = spawn(command, arguments_, {
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });

  try {
    await waitForReady(
      child,
      () => stdout,
      () => stderr,
    );
    await hooks.afterDirectoryOpened?.();
    child.stdin.end(contents, "utf8");
    await waitForExit(child, () => stderr);
  } catch (error) {
    child.stdin.destroy();
    child.kill("SIGTERM");
    throw error;
  }
}

async function waitForReady(child, stdout, stderr) {
  await new Promise((resolveReady, rejectReady) => {
    const inspect = () => {
      if (stdout().includes("READY\n")) {
        cleanup();
        resolveReady();
      }
    };
    const fail = (error) => {
      cleanup();
      rejectReady(error);
    };
    const exit = (code) => fail(reportError(code, stderr()));
    const cleanup = () => {
      child.stdout.off("data", inspect);
      child.off("error", fail);
      child.off("exit", exit);
    };
    child.stdout.on("data", inspect);
    child.once("error", fail);
    child.once("exit", exit);
    inspect();
  });
}

async function waitForExit(child, stderr) {
  if (child.exitCode !== null) {
    if (child.exitCode === 0) return;
    throw reportError(child.exitCode, stderr());
  }
  await new Promise((resolveExit, rejectExit) => {
    child.once("error", rejectExit);
    child.once("exit", (code) => {
      if (code === 0) resolveExit();
      else rejectExit(reportError(code, stderr()));
    });
  });
}

function reportError(code, stderr) {
  return new Error(stderr.trim() || `private report writer exited ${code}`);
}
