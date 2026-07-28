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
pnpm relevance:scale
```

The relevance commands validate the checked-in search corpus, emit the
intentionally failing empty-retrieval baseline, and generate the deterministic
10,000-tidbit performance workload under ignored `app/.data/relevance/`.

Repository policy and secret checks run from the repository root:

```bash
scripts/check-repository.sh
```
