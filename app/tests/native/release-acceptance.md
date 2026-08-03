# Packaged release acceptance

Status: **not yet run for the current release artifact**.

This checklist verifies the exact packaged `Kosh.app`, not Vite, browser fakes,
or a debug binary. Commands run from `app/` and confine test homes to direct
children of ignored `.data/release-acceptance/`. They refuse an existing
profile and never delete one.

## Prerequisites

- A verified candidate from `pnpm release:build:app`.
- No development, packaged, or installed Kosh process running.
- A real image with readable text and a representative text/scanned PDF.
- Network only for the explicit semantic-model setup step.

Use unique names in place of the examples.

## A. Clean note-first core

Before the visible walkthrough, the same exact package can run its commit-bound
startup, schema, React-root, IPC, lexical-search, and citation preflight
with both windows hidden:

```sh
pnpm release:acceptance prepare-clean preflight-YYYYMMDD
pnpm release:acceptance launch-hidden preflight-YYYYMMDD absent
pnpm release:acceptance check-core preflight-YYYYMMDD
pnpm release:acceptance launch-hidden preflight-YYYYMMDD present
```

`launch-hidden` requires a clean source tree, verifies that the package embeds
that exact HEAD, binds the receipt to the isolated profile, and exits by
itself. It does not claim menu-bar, global-shortcut, native file-dialog,
focus-restoration, or other visible observations below.

```sh
pnpm release:acceptance prepare-clean clean-YYYYMMDD
pnpm release:acceptance launch clean-YYYYMMDD
```

1. Confirm launch opens a focused, empty note without creating a database row.
2. Close the main window, reopen it from the menu-bar icon, and confirm Kosh
   owns the application menu without flicker; repeat from the Dock and the
   configured global shortcut.
3. Type a note, quit immediately with `Cmd+Q`, relaunch, and confirm every byte
   survived without an explicit Save action.
4. Use `Cmd+N` for a second titleless note. Add headings, nested lists, bold,
   italic, strike, inline/block code, and inline/block math.
5. Paste a long mixed-format document, type continuously through several
   autosave intervals, and exercise one IME/composition input. Rapidly switch
   between the two notes and confirm neither caret nor newest content regresses.
6. Use `Cmd+/` to hide and restore the sidebar. Confirm `Cmd+B` still toggles
   editor bold.
7. Add an HTTPS source URL, press `Cmd+K`, search exact words and a quoted
   phrase, then open the result. Confirm the matching block and source URL are
   cited honestly.
8. Navigate backward and forward between the two notes and Settings; confirm
   each note is represented by its own route and restores the editing caret.
9. Use Quick Add from another application, finish the note, and confirm focus
   returns to the original application.
10. Confirm Settings reports semantic search honestly while local capture and
    lexical search remain usable.
11. Quit with `Cmd+Q`.

```sh
pnpm release:acceptance check-core clean-YYYYMMDD
```

The command requires current migration heads, WAL, integrity/foreign-key
health, an active note, no pending working copies, authored titles, retired
tables, or query-history schema, and a lexical search projection. It does not
claim the UI observations above.

## B. Media and semantic journeys

Relaunch the same profile:

```sh
pnpm release:acceptance launch clean-YYYYMMDD
```

1. Paste and attach a real image, wait for OCR, search a distinctive OCR
   phrase, and open its region citation.
2. Attach the PDF, wait for extraction, search a phrase from a known page, and
   open its page citation. Exercise native-text and OCR pages when available.
3. Attach an arbitrary file and confirm its block survives edit, restart, and
   reveal/open actions.
4. Start semantic setup. While downloading/verifying, repeat note capture and
   lexical search to prove they remain available.
5. After Ready, run a paraphrase with no exact overlap and confirm the expected
   hybrid result and citation.
6. Restart once with the model unavailable/offline and confirm capture and
   lexical search still work with an honest degraded semantic state.
7. Quit with `Cmd+Q`.

```sh
pnpm release:acceptance check-journeys clean-YYYYMMDD
```

The durable check requires URL, code, math, image/OCR, PDF extraction,
searchable extracted text, and semantic embedding evidence.

## C. Restart

```sh
pnpm release:acceptance checkpoint-restart clean-YYYYMMDD
pnpm release:acceptance launch clean-YYYYMMDD
```

Inspect authored notes, attachment previews, hybrid results, citations, and
settings. Quit normally, then run:

```sh
pnpm release:acceptance check-restart clean-YYYYMMDD
```

The check requires unchanged logical counts across authored notes, working
copies, revision provenance, media, passages, FTS, and embedding state.

Repeat once after typing a unique marker and force-terminating Kosh after the
working-copy debounce but before an intentional quit. Relaunch should still
open a fresh blank note; use `Command-K` to find the recovered marker, then quit
normally and repeat the checkpoint/check pair. Perform three additional normal
launch/quit cycles to expose cleanup or startup regressions.

## D. Performance record

From the clean exact candidate commit, run:

```sh
pnpm baseline:redesign
```

Retain `.data/redesign/release-candidate-v1.performance.json`. It must pass the
hidden native-startup regression, editor initialization, input-paint, warm
search-overlay, first-result, and 10,000-note lexical budgets. The automated
native samples deliberately keep Kosh hidden and therefore do not claim shown
window or focus latency. During the visible walkthrough, measure 20 cold
launches from process start to a shown window with a focused editable caret;
the p95 target is 1,000 ms. Measure 20 already-running app reactivations
separately; the p95 target is 150 ms and cannot be inferred from a process
restart measurement.

## E. Installed application

After the isolated profile passes, install the exact candidate in
`/Applications` using `docs/RELEASE.md`. Repeat a short titleless note, Quick
Add, Command-K citation, normal quit, and reopen against the intended
production profile. No development tools are required on the installed Mac.

## F. Packaged off-site recovery

Follow `docs/OFFSITE_BACKUP.md` using a dedicated private test bucket. From a
clean exact-HEAD checkout, build the candidate and run:

```sh
KOSH_R2_CANARY_REQUIRE_PACKAGED=1 ../scripts/run-litestream-r2-canary.sh
```

This is intentionally not a pull-request lane. Retain its bounded redacted
`canary-report-v1.json`, `checkpoint-manifest-v1.json`, and
`packaged-recovery-smoke-v1.json`. The report must say `PACKAGED`, name the
exact clean commit, prove interrupted replication recovery and manifest-last
publication, restore notes/revisions/media/search/historical note citations, and
show zero remote residue. The normal startup proof uses hidden app windows and
an isolated home.

## Release record

| Field                                      | Result  |
| ------------------------------------------ | ------- |
| Version / commit / app SHA-256             | pending |
| macOS / hardware / tester / date           | pending |
| Cold launch and ephemeral empty note       | pending |
| Autosave, immediate quit, and restart      | pending |
| Long paste, IME, rapid switching, recovery | pending |
| Performance budgets and warm reactivation  | pending |
| Menu icon, Quick Add, and global shortcuts | pending |
| Rich blocks, math, lists, and sources      | pending |
| Command-K hybrid search and URL citation   | pending |
| Back/forward note navigation               | pending |
| Image OCR search and region citation       | pending |
| PDF search and page citation               | pending |
| Arbitrary attachment lifecycle             | pending |
| Semantic setup and degraded fallback       | pending |
| Packaged real-R2 clean-directory recovery  | pending |
| `/Applications` installed smoke            | pending |

Preserve each profile and failure log until the record is reviewed. A green
database check cannot substitute for an unobserved UI journey.
