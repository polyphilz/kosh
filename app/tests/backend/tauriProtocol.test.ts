import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { TauriCommand, TauriEvent, TauriWindow } from "../../src/tauriProtocol";

const appRoot = process.cwd();
const rustSourceRoot = `${appRoot}/src-tauri/src`;

describe("Tauri protocol registry", () => {
  it("matches every production invoke handler", () => {
    const lib = readFileSync(`${rustSourceRoot}/lib.rs`, "utf8");
    const handler = lib.match(
      /#\[cfg\(not\(feature = "test-support"\)\)\][\s\S]*?generate_handler!\[([\s\S]*?)\]\)/,
    )?.[1];

    expect(handler, "production generate_handler! block").toBeDefined();
    const rustCommands = handler!
      .split(",")
      .map((entry) => entry.trim().split("::").at(-1))
      .filter((entry): entry is string => Boolean(entry))
      .sort();

    expect(rustCommands).toEqual(Object.values(TauriCommand).sort());
  });

  it("matches every application event emitted by native code", () => {
    const rustEvents = readdirSync(rustSourceRoot)
      .filter((filename) => filename.endsWith(".rs"))
      .flatMap((filename) => {
        const source = readFileSync(`${rustSourceRoot}/${filename}`, "utf8");
        return [...source.matchAll(/"(?<event>kosh:\/\/[a-z-]+)"/g)].map(
          (match) => match.groups!.event!,
        );
      });

    expect([...new Set(rustEvents)].sort()).toEqual(Object.values(TauriEvent).sort());
  });

  it("matches the native window labels and configured startup windows", () => {
    const windowsSource = readFileSync(`${rustSourceRoot}/windows.rs`, "utf8");
    const rustLabels = [...windowsSource.matchAll(/const [A-Z_]+_LABEL: &str = "([^"]+)";/g)].map(
      (match) => match[1]!,
    );
    const config = JSON.parse(readFileSync(`${appRoot}/src-tauri/tauri.conf.json`, "utf8")) as {
      app: { windows: Array<{ label: string }> };
    };

    expect(rustLabels.sort()).toEqual(Object.values(TauriWindow).sort());
    expect(config.app.windows.map(({ label }) => label)).toEqual([TauriWindow.Main]);
  });
});
