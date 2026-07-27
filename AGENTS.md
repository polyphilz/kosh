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
- Commit messages are one succinct, descriptive line.
- Never commit `.env`, credentials, tokens, local databases, model weights, or
  `.kosh-loop/` state.
- Open pull requests ready for review and include scope, acceptance criteria,
  and verification in the body.
- Address valid Codex review findings on the same branch. Record the rationale
  for invalid findings without making a token code change.
- Never invoke `gh pr merge` directly. Use `scripts/loop/merge.sh`, which
  requires current-head CI and Codex review evidence before squash-merging.

## Verification

Until the application toolchain is scaffolded, run:

```bash
scripts/check-repository.sh
```

Once `app/package.json` exists, prefer the repository's aggregate `pnpm check`
command plus targeted Rust or UI tests for the active slice.

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
