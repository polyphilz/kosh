import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

type TauriConfig = {
  app: {
    security: {
      freezePrototype: boolean;
      capabilities: string[];
      csp: Record<string, string>;
      devCsp: Record<string, string>;
    };
  };
  bundle: {
    resources?: Record<string, string>;
  };
};

const json = <T>(path: string): T => JSON.parse(readFileSync(path, "utf8")) as T;
const production = json<TauriConfig>("src-tauri/tauri.conf.json");
const release = json<TauriConfig>("src-tauri/tauri.release.conf.json");
const defaultCapability = json<{ windows: string[]; permissions: string[] }>(
  "src-tauri/capabilities/default.json",
);
const quickAddCapability = json<{ windows: string[]; permissions: string[] }>(
  "src-tauri/capabilities/quick-add.json",
);

describe("desktop security boundary", () => {
  it("keeps production navigation, execution, framing, and IPC closed by default", () => {
    const security = production.app.security;
    expect(security.freezePrototype).toBe(true);
    expect(security.capabilities.toSorted()).toEqual(["default", "quick-add"]);
    expect(security.csp).toEqual({
      "default-src": "'self'",
      "connect-src": "ipc: http://ipc.localhost",
      "img-src": "'self' blob: data: kosh-media:",
      "style-src": "'self' 'unsafe-inline'",
      "object-src": "kosh-media:",
      "frame-src": "'none'",
      "base-uri": "'none'",
      "form-action": "'none'",
    });
    expect(JSON.stringify(security.csp)).not.toMatch(
      /unsafe-eval|unsafe-hashes|https?:\/\/\*|wss?:\/\/\*|\bfile:/u,
    );
    expect(security.devCsp["connect-src"]).toBe(
      "'self' ipc: http://ipc.localhost ws://127.0.0.1:1420",
    );
  });

  it("grants only core access and bounded note-link ingress to the main window", () => {
    expect(defaultCapability).toMatchObject({
      windows: ["main"],
      permissions: ["core:default", "deep-link:default"],
    });
    expect(quickAddCapability).toMatchObject({
      windows: ["quick-add"],
      permissions: ["core:default"],
    });
  });

  it("bundles only pinned semantic and recovery runtime resources", () => {
    expect(Object.keys(release.bundle.resources ?? {}).toSorted()).toEqual([
      "resources/embedding-indexes/jina-v1-golden.json",
      "resources/embedding-indexes/jina-v1.json",
      "resources/release/bin/litestream",
      "resources/release/bin/llama-server",
      "resources/release/licenses/litestream-LICENSE",
      "resources/release/licenses/litestream-NOTICE",
      "resources/release/licenses/llama.cpp-LICENSE",
      "resources/release/litestream.json",
      "resources/release/llama-server.json",
      "resources/release/source.json",
    ]);
    expect(JSON.stringify(release.bundle.resources)).not.toMatch(
      /\.env|\.sqlite|\.gguf|\.onnx|\.safetensors|test-results|\.kosh-loop/u,
    );
  });

  it("registers one typed local-media protocol and no arbitrary URL opener plugin", () => {
    const source = readFileSync("src-tauri/src/lib.rs", "utf8");
    expect(source.match(/register_uri_scheme_protocol\(/gu)).toHaveLength(1);
    expect(source).toContain('.register_uri_scheme_protocol("kosh-media"');
    expect(source).not.toMatch(/tauri_plugin_(shell|opener|fs|http)/u);
  });

  it("routes every externally callable media reclamation through a safety snapshot", () => {
    const media = readFileSync("src-tauri/src/media.rs", "utf8");
    const writer = readFileSync("src-tauri/src/database/writer.rs", "utf8");
    expect(media).toContain("maintain_media_with_safety_snapshot(now_ms, limits)");
    expect(media).not.toContain("client.maintain_media(now_ms, limits)");
    expect(writer).not.toContain("WriterMessage::MaintainMedia {");
    expect(writer).not.toContain("pub fn maintain_media(");
  });

  it("pins hardening bundle evidence against inherited environment overrides", () => {
    const report = readFileSync("../scripts/run-hardening-report.sh", "utf8");
    expect(report).toContain('KOSH_BUNDLE_ROOT="$app_root/dist"');
    expect(report).toContain('KOSH_BUNDLE_REPORT="$bundle_report"');
    expect(report).toContain('rm -- "$bundle_report"');
    const publication = report.slice(report.lastIndexOf('>"$temporary"'));
    expect(publication).toContain('[[ "$(git -C "$repo_root" rev-parse HEAD)" == "$head_sha" ]]');
    expect(publication).toContain(
      'git -C "$repo_root" status --porcelain --untracked-files=normal',
    );
    expect(publication.indexOf("rev-parse HEAD")).toBeLessThan(
      publication.indexOf('mv "$temporary"'),
    );
    expect(publication.indexOf("status --porcelain")).toBeLessThan(
      publication.indexOf('mv "$temporary"'),
    );
  });

  it("keeps retired agent surfaces out of production source", () => {
    expect(existsSync("src-tauri/src/claude.rs")).toBe(false);
    expect(existsSync("src-tauri/src/research")).toBe(false);
    expect(existsSync("src/routes/ResearchPage.tsx")).toBe(false);
  });
});
