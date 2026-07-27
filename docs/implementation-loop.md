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

## Review completion contract

`scripts/loop/gate.sh` is the merge authority. For an open, ready PR targeting
`main`, it requires:

1. at least one CI check and no failed, pending, or canceled checks;
2. a user-authored `@codex review` request created after a GitHub Actions run
   records the current head on GitHub;
3. a later `+1` reaction from `chatgpt-codex-connector[bot]` on that exact
   request, which is Codex's clean-review signal; and
4. a mergeable GitHub state.

The server-side workflow timestamp prevents author-controlled Git timestamps
or a reaction from an older revision from authorizing a newer revision.
`scripts/loop/merge.sh` runs the gate, binds the merge atomically to that head
SHA, and uses the repository's squash-only merge policy.

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
forms. It is a backstop, not permission to place secrets in source files.
