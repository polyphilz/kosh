# Kosh agent guidance

## Product invariants

- Kosh is a macOS-first, local-first Tauri application.
- Kosh is a titleless, note-first capture tool: cold launch, typing, durable
  autosave, hybrid search, and exact passage navigation are its critical path.
- Capture and lexical search must remain usable when the embedding model or
  remote backup is unavailable.
- Search results operate on citation-sized passages. A trusted citation must
  resolve to the exact stored revision, attachment page, OCR evidence, or text
  line range supplied for that result.
- Research is a retired product surface. The redesign is a hard cutover with
  no deployed profiles to migrate, so do not retain its schema, rows, runtime
  adapters, fixtures, or migrations for compatibility.
- Tidbit revisions and media blobs are immutable. Background extraction and
  embedding work must be content-hash checked before stale results can install.
- R2 is single-writer backup/recovery, not multi-device synchronization.

## Repository workflow

- `.plans/001-impl.md` remains the implementation foundation;
  `.plans/003-redesign.md` supersedes its product and UI direction where they
  conflict.
- Work on one reviewable slice, branch, and pull request at a time.
- Redesign branches use `polyphilz/redesign-<zero-based-slice>-<short-description>`.
- Keep changes scoped to the active slice and preserve unrelated user work.
- Run targeted verification and the complete available check suite before
  committing.
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
the exact committed head must launch from a fresh profile and restart against
that profile without losing a searchable cited canary. Every native launch must
also load both exact-head frontend entries, render their React roots, and
complete an IPC probe against the same runtime data directory before it can
produce a passing receipt.

The comprehensive test architecture lives in the ignored
`.plans/002-testing.md`; the redesign matrix and rollout live in
`.plans/003-redesign.md`. Redesign Chunk 13 executes the complete browser,
native, migration, relevance, durability, security, performance, and release
matrix; earlier chunks add the targeted layers called out for their changed
contract.

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
