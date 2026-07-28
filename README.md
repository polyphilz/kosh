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

Repository policy and secret checks run from the repository root:

```bash
scripts/check-repository.sh
```
