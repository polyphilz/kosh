# Implementation loop

Kosh is implemented as a sequence of reviewable slices derived from the local
`.plans/001-impl.md`. The plan is intentionally ignored, so each pull request
copies its slice scope, acceptance criteria, and verification into the PR body.
Repository-wide invariants live in the tracked `AGENTS.md`.

## State machine

```text
select
  -> implement
  -> verify
  -> commit
  -> verify exact-head fresh/restart/preserved launches
  -> push
  -> open PR
  -> wait for Codex
       -> valid finding: fix -> verify -> commit -> push -> review again
       -> invalid finding: record rationale -> review again
       -> clean current-head review: gate -> squash merge -> next slice
```

Only one branch and pull request may be active at a time. Runtime progress and
feedback dispositions belong under ignored `.kosh-loop/`; they must not create
commits that invalidate an in-flight review.

## Progressive operability contract

Every committed slice must run as an application, not merely compile as a
collection of components. `scripts/loop/runtime-gate.sh` launches the real
debug Tauri binary three times:

1. against a unique empty data root, where it migrates both databases, creates
   a canary tidbit through the production writer, finds it through exact lexical
   search, and resolves its source-bearing citation;
2. against that same root, where the canary must already exist and resolve to
   the identical tidbit revision and passage; and
3. against `.kosh-loop/progressive-profile/data`, where the canary and database
   pair from prior slices must survive all forward migrations.

Each launch also starts the exact-head Vite frontend and proves that the main
and quick-add windows are constructed, both React roots render, both surfaces
load from the gate-owned pinned IPv4 origin, both complete a distinct Tauri IPC
probe against the expected data directory, both database files use WAL and
foreign keys, and both embedded migration heads are applied. A blank, stale,
or error webview therefore cannot issue a passing receipt. The fresh profile is
disposable. The preserved profile is not: never delete, replace, or edit it to
make a slice pass. On a genuinely new workstation, initialize it exactly once
with:

```bash
scripts/loop/runtime-gate.sh --bootstrap-persistent
```

Bootstrap is recoverable: the gate first establishes an owned staging profile,
idempotently ensures its canary, atomically promotes the profile, and launches
the promoted path again before marking it established. An interrupted
bootstrap resumes only this gate-owned state; it never asks the operator to
delete a database whose provenance is uncertain.

Normal slice verification uses `scripts/loop/runtime-gate.sh` with no flags.
The script requires a clean worktree and writes an ignored, exact-commit
receipt to `.kosh-loop/runtime-gate.json`. A later commit invalidates that
receipt. CI independently launches and restarts a fresh profile on macOS with
`scripts/loop/runtime-gate.sh --ci`.

## Review completion contract

`scripts/loop/gate.sh` is the merge authority. For an open, ready PR targeting
`main`, it requires:

1. a clean local worktree whose `HEAD` is the pull request head and whose
   progressive runtime receipt validates for that exact commit;
2. at least one CI check and no failed, pending, or canceled checks;
3. a user-authored `@codex review` request created after the latest GitHub
   Actions run records a transition to the current head on GitHub;
4. one later clean-review signal from `chatgpt-codex-connector[bot]`, either:
   - a `+1` reaction on that exact review-request comment; or
   - a clean completion comment naming the exact current head SHA; and
5. a mergeable GitHub state.

The server-side workflow timestamp prevents author-controlled Git timestamps
or clean evidence from an older revision from authorizing a newer revision. A
PR-body reaction is informational only because GitHub does not bind it to a
request or commit.
`scripts/loop/merge.sh` runs the gate, binds the merge atomically to that head
SHA, rechecks that local source state did not change, and uses the repository's
squash-only merge policy.

## Review feedback

Review findings are assessed against the implementation plan, repository
invariants, tests, and the changed code:

- Valid findings are fixed on the same branch and receive focused regression
  coverage where practical.
- Invalid findings receive a rationale in `.kosh-loop/events.jsonl`; no code is
  changed merely to make a new commit.
- After either outcome, request review of the current head again.
- If the same disputed finding repeats twice and prevents a clean review, stop
  for user judgment rather than weakening the gate.

## Secret handling

Real development credentials belong only in ignored `.env`. `.env.example`
documents variable names with blank credential values. Packaged application
credentials will move to macOS Keychain when backup work is implemented.

`scripts/check-secrets.sh` rejects tracked environment files and common secret
forms in the worktree, index, and every commit introduced since the CI base.
It reports only affected paths so CI logs do not repeat credential values. It
is a backstop, not permission to place secrets in source files.
