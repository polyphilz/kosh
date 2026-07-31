# Releasing Kosh

This is the authoritative runbook for a Kosh personal release. Build from a
clean, reviewed `main` checkout. Ordinary `pnpm tauri build` is not a release
build because it does not stage or verify the pinned native sidecars.

## Current distribution policy

- The supported artifact is a universal `Kosh.app` for macOS 14 or newer.
- The app and bundled universal `llama-server` and `litestream` executables are
  ad-hoc signed. This is a local/personal artifact, not a notarized public
  download.
- The release has explicit empty entitlements, no updater, no DMG, and no
  automatic tag-to-release workflow.
- The 232,883,776-byte embedding model is not bundled. Capture and lexical
  search work without it; Kosh downloads and verifies it only after explicit
  semantic setup.
- Research is optional. It uses a separately installed and authenticated
  `claude` CLI and never enables web search.
- Production data is outside the app bundle under macOS Application Support.
  Replacing `Kosh.app` must never replace or reset that data.

The ignored source artwork under `assets/` is not part of the release.
Generated app icons, the web mark, and the monochrome 32×32 tray template are
tracked build inputs.

## Prerequisites

The build Mac needs Node 22.12.0, pnpm 10.4.0, Rust 1.97.0, Xcode command-line
tools, CMake, an Apple Silicon host, and Rosetta. Install both Rust targets:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

Place the pinned model at:

```text
models/v5-nano-retrieval-Q8_0.gguf
```

Its expected size and SHA-256 are pinned in
`app/src-tauri/resources/embedding-indexes/jina-v1.json`. This copy verifies
both sidecar architectures through CPU and Metal; it is never copied into the
app.

## 1. Verify the source

The same version must appear in `app/package.json`,
`app/src-tauri/Cargo.toml`, and `app/src-tauri/tauri.conf.json`.

From `app/`, run:

```sh
pnpm check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml \
  --all-targets --all-features -- -D warnings
pnpm relevance:gate
pnpm relevance:lexical-scale
pnpm check:bundle
pnpm release:migration
pnpm release:verify-contracts
```

From the repository root, run `scripts/check-repository.sh`. The branch must
then pass the normal runtime, CI, Codex-review, and guarded merge gates. Never
release a dirty worktree or an unreviewed commit.

## 2. Build and verify the app

From a clean `main` checkout:

```sh
cd app
pnpm install --frozen-lockfile
pnpm release:build:app
pnpm release:smoke
```

The build:

1. verifies identity, version, icon, CSP, capabilities, entitlements,
   resources, and signing policy;
2. checks out the exact pinned llama.cpp revision;
3. builds arm64 and x86_64 `llama-server` slices with embedded Metal shaders;
4. runs the pinned model/golden fixtures on CPU and Metal for both slices;
5. verifies both official Litestream archives and their upstream checksum
   file, then assembles the pinned universal `litestream`;
6. rejects non-system dependencies and verifies every sidecar slice;
7. stages only binaries, generated manifests, licenses/notices, and embedding
   contracts;
8. builds a universal Tauri application;
9. ad-hoc signs the app and nested executables; and
10. checks architectures, hashes, versions, executable bits, Info.plist,
    signatures, exact resources, and bundle isolation.

The verified artifact is:

```text
app/src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app
```

`release:smoke` starts that exact bundle twice with a temporary isolated home
and Finder-like minimal `PATH`. The packaged React root creates a URL-bearing
canary through normal capture IPC, both packaged surfaces resolve its exact
search citation, and the second launch proves the same cited revision survives.
The native executable embeds its source commit, and the lane rejects a bundle
whose embedded commit differs from the checkout under test. Release build and
smoke commands also reject modified, staged, or untracked source, while ignored
local data remains allowed. The lane also checks current migrations,
WAL/integrity, and operation without Claude or a semantic model.

## 3. Run release acceptance

The checklist's `launch-hidden` preflight can verify the exact packaged commit,
migrations, both rendered webviews, IPC, restart, Exact search, and citations
without foregrounding Kosh. It is useful while the Mac is in active use, but it
does not replace the checklist's visible menu, shortcut, file-dialog, focus,
or `/Applications` observations.

Follow
[`app/tests/native/release-acceptance.md`](../app/tests/native/release-acceptance.md)
against the exact candidate. The profiles live beneath ignored
`app/.data/release-acceptance/`; commands reject existing or out-of-scope
profiles and never point the candidate at production data.

The required record covers clean capture and lexical search without
Claude/model, Quick Add, code/math, image OCR, PDF search, hybrid search,
grounded Research citations, restart, previous-release migration, a verified
pre-migration snapshot, and restoration into a separate replacement profile.
Do not claim a UI journey from database checks alone.

If off-site recovery is configured for the release, also complete the packaged
real-R2 lane in
[`docs/OFFSITE_BACKUP.md`](OFFSITE_BACKUP.md). Its redacted receipt must name
the exact clean source commit and prove a clean-directory restore through the
packaged recovery command plus a hidden normal startup of that restored
library.

## 4. Archive and install

Set the version explicitly and archive outside the repository:

```sh
KOSH_RELEASE_VERSION=0.1.0
KOSH_RELEASE_APP=src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app
KOSH_RELEASE_ARCHIVE="$HOME/Downloads/Kosh-$KOSH_RELEASE_VERSION-macos-universal.zip"

ditto -c -k --sequesterRsrc --keepParent \
  "$KOSH_RELEASE_APP" \
  "$KOSH_RELEASE_ARCHIVE"
shasum -a 256 "$KOSH_RELEASE_ARCHIVE"
```

Record the source commit and archive SHA-256 together. Quit Kosh with `Cmd+Q`,
drag the candidate into `/Applications`, and choose **Replace** for an
upgrade. Launch `/Applications/Kosh.app` from Finder, Spotlight, or Raycast.

Because this personal build is not notarized, a downloaded copy may require
Finder's **Open** confirmation or System Settings → Privacy & Security →
**Open Anyway**. For a personally trusted and checksum-verified archive,
removing quarantine is an explicit operator alternative:

```sh
xattr -cr /Applications/Kosh.app
```

Never bypass Gatekeeper for an artifact whose source and checksum are unknown.

## 5. Installed smoke check

Use the copy under `/Applications`, not the build directory.

- Confirm only the white Kosh mark appears in the menu bar; there is no text.
- Exercise the configurable Quick Add and main-window shortcuts.
- Create a URL-bearing tidbit containing code and math.
- Run exact lexical search and open its resolved citation.
- Add a real image and PDF; wait for extraction and search their text.
- Prepare semantic search, run a paraphrase, and open the exact cited passage.
- If Claude is configured, run Research and open every citation.
- Quit with `Cmd+Q`, reopen, and confirm all durable content survives.
- Confirm capture and lexical search still work with Claude unavailable and
  while semantic setup is unavailable.
- Quit and confirm no Kosh, `llama-server`, `litestream`, PDF worker, or Claude
  subprocess remains.

GUI apps do not inherit a login shell's `PATH`. Kosh probes
`~/.local/bin/claude`, `~/.claude/local/claude`,
`/opt/homebrew/bin/claude`, and `/usr/local/bin/claude` before inherited
`PATH`. The packaged acceptance launcher supplies a minimal path and exposes
an existing CLI through the first home-local location. Its
`--without-claude` mode sets an explicit discovery disable and is mandatory for
the clean-core lane.

## Rollback and recovery boundary

Reinstalling an older app does not roll back a migrated database. If an
upgrade fails, quit Kosh and preserve the failed Application Support directory
plus logs. Inspect the newest `safety-snapshots/migration-*` manifest before
copying anything. The acceptance recovery command operates only on ignored
test profiles; it is not authorization to overwrite production data.

The package contains the pinned Litestream executable, the opt-in
single-writer backup runtime, exact checkpoint preview/drill controls, and the
offline clean-directory recovery command documented in
[`docs/OFFSITE_BACKUP.md`](OFFSITE_BACKUP.md). It is disaster recovery, not
multi-device sync. Turning backup off preserves remote objects; v1 does not
automatically delete immutable checkpoint manifests or media.

## Public distribution is future work

Before publishing Kosh as a normal download, add Developer ID signing for
every executable, hardened-runtime entitlement validation, notarization and
stapling, a signed installer/DMG, clean-machine Gatekeeper testing, and a
release workflow that preserves every sidecar/model verification gate.
