# Kosh

Kosh is a macOS-first, local-first note taker for capturing loose tidbits and
finding the exact passage that matters.

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
`KOSH_DATA_DIR` to `app/.data/local`, keeping test notes away from Tauri's
release app-data directory. The Rust backend honors this override only in debug
builds; release builds always use the platform app-data directory.

## Global capture

Kosh launches as a menu-bar resident macOS app. Use `⌃⌥⌘K` to open the
persistent Quick Add window from any application and `⌃⌥⌘O` to bring the main
window forward. Both shortcuts are configurable in Settings. Saving with
`⌘↵` or explicitly cancelling Quick Add restores the application that was
active before capture; clicking away hides the window while preserving its
local draft and attachment leases.

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
```

The relevance commands validate the checked-in search corpus, emit the
intentionally failing empty-retrieval baseline, record the current lexical
baseline, and generate the deterministic 10,000-tidbit performance workload
under ignored `app/.data/relevance/`. The release-mode lexical benchmark uses a
real migrated, WAL-backed Kosh library and the production write, FTS,
authoritative hydration, ranking, and citation-resolution paths. It enforces a
100 ms p95 interactive budget and writes machine/runtime metadata beside its
ignored report.

## Lexical search

The native backend projects current citation passages into separate word and
trigram FTS5 indexes. Search covers title, heading context, passage body,
source labels and URLs/domains, attachment filenames, and extracted text.
Queries are parsed as literal data before FTS execution; quoted phrases and
explicit Exact mode never forward raw user syntax into `MATCH`.

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

Repository policy and secret checks run from the repository root:

```bash
scripts/check-repository.sh
```
