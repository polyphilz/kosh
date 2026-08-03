# Kosh

Kosh is a macOS-first, local-first note taker for capturing loose tidbits and
finding the exact passage that matters.

## Using Kosh

Kosh opens directly into a focused blank note. The note remains ephemeral until
you type or add media, then saves automatically; there is no title field or Save
button. The editor supports paragraphs, H1-H3, nested ordered and unordered
lists, bold, italic, strikethrough, inline and fenced code, inline and display
math, images, PDFs, and files.

- `⌘N` starts another blank note.
- `⌘K` opens hybrid search. Lexical results remain available when the optional
  semantic model is not ready.
- `⌘/` toggles the sidebar. `⌘B` remains editor bold.
- Back/forward navigation and the macOS backswipe move between visited notes.
- `⌘Q` waits for the active note to become durable; a failed save cancels quit
  and leaves a visible retry state.

Search results are passage-level citations. Opening one navigates to its note,
focuses the matching block or attachment, and preserves the source URL when the
note has one. Kosh stores no query or search-history feature.

## Development

Install Node 22.12.0, pnpm 10.4.0, Rust 1.97.0, and the Xcode command line
tools. The repository pins the language toolchains; pnpm is pinned in
`app/package.json`.

```bash
cd app
pnpm install --frozen-lockfile
pnpm exec playwright install chromium
pnpm tauri dev
```

Always start the development app through `pnpm tauri dev`. That script sets
`KOSH_DATA_DIR` to `app/.data/note-first-v1`, keeping test notes away from
Tauri's release app-data directory. The note-first hard cutover leaves the
former `app/.data/local` profile untouched because it is incompatible with the
consolidated schema. The Rust backend honors this override only in debug builds;
release builds always use the platform app-data directory.

## Global capture

Kosh launches as a regular macOS app and remains available from its Dock and
menu-bar icon after the main window closes. Use `⌃⌥⌘K` to open the persistent
Quick Add window from any application and `⌃⌥⌘O` to bring the main window
forward. Both shortcuts are configurable in Settings. Saving with `⌘↵` or
explicitly cancelling Quick Add restores the application that was active
before capture; clicking away hides the window while preserving its local
draft and attachment leases.

## Checks

From `app/`, run the complete local suite:

```bash
pnpm check
```

Focused commands are also available:

```bash
pnpm lint
pnpm fmt:check
pnpm build
pnpm test
pnpm test:browser
pnpm check:native
pnpm relevance:validate
pnpm relevance:empty
pnpm relevance:lexical
pnpm relevance:scale
pnpm relevance:lexical-scale
pnpm acceptance:redesign
pnpm hardening:report
```

`pnpm acceptance:redesign` is the final redesign gate and writes its
commit-bound performance report beneath ignored `app/.data/redesign/`.
`pnpm hardening:report` is also commit-bound; both require a clean worktree.

The relevance commands validate the checked-in search corpus, emit the
intentionally failing empty-retrieval baseline, record the current lexical
baseline, and generate the deterministic 10,000-note performance workload
under ignored `app/.data/relevance/`. The release-mode lexical benchmark uses a
fresh WAL-backed Kosh database and the production write, FTS,
authoritative hydration, ranking, and citation-resolution paths. It enforces a
100 ms p95 interactive budget and writes machine/runtime metadata beside its
ignored report.

## Supported scale and limits

Kosh's v1 target is a 10,000-note local library. The release-mode lexical
gate must keep interactive query latency at or below 100 ms p95 on its
deterministic 200-query workload. Each working copy supports up to 32 attachments;
each direct attachment, source image, or PDF may be up to 32 MiB. PDFs may
contain up to 2,000 pages, with OCR bounded to 128 image-only pages. Searchable
text extraction reads at most 4 MiB and 5,000 passages per attachment.

See [docs/hardening.md](docs/hardening.md) for the complete performance,
recovery, security, accessibility, and supported-input matrix, plus the
reproducible hardening report command.

## Search and citations

The native backend projects current citation passages into separate word and
trigram FTS5 indexes. Search covers heading context, passage body,
source labels and URLs/domains, attachment filenames, and extracted text.
Queries are parsed as literal data before FTS execution; quoted phrases and
safe internal literal matching never forward raw user syntax into `MATCH`.

Results carry Kosh-resolved citation snapshots, matched field names, and
character-offset highlight spans. Edits, soft deletion, and restoration update
the active search projection in the same database transaction while historical
passages remain resolvable as citations.

## Local semantic runtime

Semantic search uses the pinned
`jinaai/jina-embeddings-v5-text-nano-retrieval-GGUF` Q8 model through a pinned
`llama.cpp` sidecar. The 232,883,776-byte model is not bundled and is never
downloaded merely by launching Kosh or running its tests. An explicit prepare
request downloads it resumably beneath Kosh's data directory, verifies SHA-256
`86b6e6279e9b9e71389f02a082764a2ac2b15a50e37482c26f98d69092f12442`,
and checks both query and document golden vectors before semantic retrieval may
activate. Lexical search remains available in every missing, download, verify,
or runtime-failure state.

Development can use already verified local artifacts:

```bash
cd app
KOSH_EMBEDDING_MODEL_PATH="$PWD/../models/v5-nano-retrieval-Q8_0.gguf" \
KOSH_LLAMA_SERVER_PATH=/opt/homebrew/bin/llama-server \
  pnpm tauri dev
```

The versioned model, golden-vector, and sidecar contracts live under
`app/src-tauri/resources/`. To reproduce the compatibility gate against a
local llama.cpp build:

```bash
LLAMA_EMBEDDING=/path/to/llama-embedding \
LLAMA_SERVER=/path/to/llama-server \
  scripts/verify-jina-v1.sh models/v5-nano-retrieval-Q8_0.gguf
```

`scripts/build-llama-sidecar.sh` checks out the exact pinned llama.cpp
revision, builds separate arm64 and x86_64 static binaries, combines them into
a universal macOS sidecar, and explicitly runs both architecture slices through
CPU and Metal golden checks. The release build therefore requires an Apple
Silicon Mac with Rosetta installed. It also checks each slice's dynamic
dependencies and writes only to the ignored release staging directory. Release
packaging is intentionally separate from ordinary and test builds:

```bash
cd app
pnpm release:build
```

See [docs/RELEASE.md](docs/RELEASE.md) for universal artifact verification,
packaged-app acceptance, Developer ID signing, notarization, signed GitHub
updater releases, installation, and rollback.

## Litestream protocol foundation

Kosh ships a pinned universal Litestream executable as the foundation for the
optional single-writer R2 disaster-recovery feature. Backup is not enabled by
the protocol slice and is never a startup dependency. The official archives,
checksums, exact-TXID fence, compaction behavior, full-integrity restore,
graceful shutdown, and zero-residue real-R2 spike are documented in
[app/tests/native/litestream-protocol.md](app/tests/native/litestream-protocol.md).
Capture and lexical search remain completely local when Litestream,
credentials, or the network are unavailable.

The persistence/R2 boundary stores only non-secret configuration, keeps R2
keys in macOS Keychain, derives only Cloudflare endpoints, and confines
production objects to a fixed per-backup-set namespace. Its fake and live
object-store probes are documented in
[app/tests/native/offsite-backup-foundation.md](app/tests/native/offsite-backup-foundation.md).

Enabled backups durably reconcile referenced source and preview blobs through
a create-only background uploader that never performs network I/O on the
authored database writer. Crash, offline retry, and metadata-verification
contracts are documented in
[app/tests/native/offsite-backup-media.md](app/tests/native/offsite-backup-media.md).

The relational database is replicated only for an enabled configuration by a
supervised Litestream child. Kosh writes a private fixed-protocol config,
validates the private control socket, owns stale-process cleanup through a
bound PID record, reports only bounded status codes, restarts transient
failures with capped backoff, and gives Litestream its verified graceful final
sync window during shutdown. Missing credentials, the helper binary, or the
network degrade backup without delaying local startup or authored writes. See
[app/tests/native/offsite-backup-litestream-runtime.md](app/tests/native/offsite-backup-litestream-runtime.md).

Repository policy and secret checks run from the repository root:

```bash
scripts/check-repository.sh
```
