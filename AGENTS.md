# Kosh agent guidance

## Product invariants

- Kosh is a macOS-first, local-first Tauri application.
- Capture and lexical search must remain usable when Claude, the embedding
  model, or remote backup is unavailable.
- Search results operate on citation-sized passages. A trusted citation must
  resolve to the exact stored revision, attachment page, OCR evidence, or text
  line range supplied for that result.
- `claude -p` research is read-only and has no web tools in v1. Never trust an
  agent-provided URL or identifier as a citation target; resolve opaque handles
  through Kosh-owned data.
- Tidbit revisions and media blobs are immutable. Background extraction and
  embedding work must be content-hash checked before stale results can install.
- R2 is single-writer backup/recovery, not multi-device synchronization.

## Repository workflow

- The local implementation source of truth is `.plans/001-impl.md`.
- Work on one reviewable slice, branch, and pull request at a time.
- Branches use `codex/<slice-number>-<short-description>`.
- Keep changes scoped to the active slice and preserve unrelated user work.
- Run targeted verification and the complete available check suite before
  committing.
- After committing, run `scripts/loop/runtime-gate.sh` for the exact head before
  pushing. On a new workstation only, establish the preserved profile with
  `scripts/loop/runtime-gate.sh --bootstrap-persistent`.
- Commit messages are one succinct, descriptive line.
- Never commit `.env`, credentials, tokens, local databases, model weights, or
  `.kosh-loop/` state.
- Open pull requests ready for review and include scope, acceptance criteria,
  and verification in the body.
- Address valid Codex review findings on the same branch. Record the rationale
  for invalid findings without making a token code change.
- Never invoke `gh pr merge` directly. Use `scripts/loop/merge.sh`, which
  requires an exact-head runtime receipt, current-head CI, and Codex review
  evidence before squash-merging.

## Verification

Until the application toolchain is scaffolded, run:

```bash
scripts/check-repository.sh
```

Once `app/package.json` exists, prefer the repository's aggregate `pnpm check`
command from `app/` plus targeted Rust or UI tests for the active slice.
Every slice must also leave the real Tauri application progressively operable:
the exact committed head must launch from a fresh profile, restart against that
profile without losing a searchable cited canary, and launch against the
preserved `.kosh-loop/progressive-profile/` created by the preceding slices.
Every native launch must also load both exact-head frontend entries, render
their React roots, and complete an IPC probe against the same runtime data
directory before it can produce a passing receipt.
The runtime gate owns these profiles; never replace or clear them to make a
migration failure pass.

The comprehensive test architecture and rollout live in the ignored
`.plans/002-testing.md`. Chunk 26 executes the complete browser, native,
migration, relevance, durability, security, performance, and release matrix;
earlier chunks add the targeted layers called out for their changed contract.

## Code Review Rules

### Citation integrity

- Flag any path that lets search or agent output display a citation whose
  target was not resolved from Kosh-owned passage and provenance data.
- Historical citations must remain attached to the revision that was actually
  used; edits must not silently retarget them.

### Durable background work

- Flag extraction, OCR, indexing, or embedding writes that can install results
  without verifying the current content hash and index/extractor version.
- Crash recovery must be idempotent and must not turn optional derived data
  into a startup dependency for authored content.

### Media safety

- Flag arbitrary filesystem exposure, unbounded media allocation, unsafe URL
  schemes, or deletion of content-addressed blobs without an explicit
  reference check and reaping authorization.
