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
- An authenticated Claude CLI only for Research. Core acceptance must pass
  without it.

Use unique names in place of the examples.

## A. Clean core without Claude or semantic setup

Before the visible walkthrough, the same exact package can run its commit-bound
startup, migration, React-root, IPC, Exact-search, and citation preflight with
both windows hidden:

```sh
pnpm release:acceptance prepare-clean preflight-YYYYMMDD
pnpm release:acceptance launch-hidden preflight-YYYYMMDD absent
pnpm release:acceptance check-core preflight-YYYYMMDD
pnpm release:acceptance launch-hidden preflight-YYYYMMDD present
```

`launch-hidden` requires a clean source tree, verifies that the package embeds
that exact HEAD, disables Claude, binds the receipt to the isolated profile,
and exits by itself. It does not claim menu-bar, global-shortcut, native file
dialog, focus-restoration, or other visible observations below.

```sh
pnpm release:acceptance prepare-clean clean-YYYYMMDD
pnpm release:acceptance launch clean-YYYYMMDD --without-claude
```

The launcher uses an isolated home and GUI-like `/usr/bin:/bin` path. The
explicit flag also disables standard-location discovery, so an installed host
Claude cannot leak into the core lane. Before preparing semantic search or
invoking Research:

1. Confirm launch opens the main window and shows Kosh in both the Dock and menu bar.
2. Close the main window, reopen it from the menu-bar icon, and confirm Kosh owns the application menu without flicker; repeat from the Dock and configured global shortcut.
3. Use Quick Add from another application, save, and confirm focus returns.
4. Create a longer tidbit with a heading, fenced code block, inline/display
   math, and an HTTPS source URL.
5. Create an offhand one-line tidbit.
6. Search exact words and a quoted phrase; open the citation and confirm it
   targets the correct revision, passage, and source URL.
7. Confirm Settings honestly reports semantic search and Research as
   unavailable/not prepared while capture and lexical search remain usable.
8. Quit with `Cmd+Q`.

```sh
pnpm release:acceptance check-core clean-YYYYMMDD
```

The command requires current migration heads, WAL, integrity/foreign-key
health, an active tidbit, and a lexical search projection. It does not claim
the UI observations above.

## B. Media, semantic, and Research journeys

Relaunch the same profile:

```sh
pnpm release:acceptance launch clean-YYYYMMDD
```

1. Attach the real image, wait for OCR, search a distinctive OCR phrase, and
   open its region citation.
2. Attach the PDF, wait for extraction, search a phrase from a known page, and
   open its page citation. Exercise native-text and OCR pages when available.
3. Start semantic setup. While downloading/verifying, repeat capture and
   lexical search to prove they remain available.
4. After Ready, run a paraphrase with no exact overlap and confirm the expected
   hybrid result and citation.
5. Restart once with the model unavailable/offline and confirm capture and
   lexical search still work with an honest degraded semantic state.
6. If Claude is configured, run a multi-passage Research query. Open every
   citation, confirm all claims bind to issued local evidence, and save the
   answer as a tidbit.
7. Quit with `Cmd+Q`.

```sh
pnpm release:acceptance check-journeys clean-YYYYMMDD
```

The durable check requires URL, code, math, image/OCR, PDF extraction,
searchable extracted text, semantic embedding, and completed grounded
Research citation evidence. If Claude is intentionally absent, record the
core/media/semantic observations separately and leave Research pending; do not
weaken the command.

## C. Restart

```sh
pnpm release:acceptance checkpoint-restart clean-YYYYMMDD
pnpm release:acceptance launch clean-YYYYMMDD
```

Inspect authored notes, attachment previews, exact/hybrid results, citations,
settings, and Research history. Quit normally, then run:

```sh
pnpm release:acceptance check-restart clean-YYYYMMDD
```

The check requires unchanged logical counts across authored, media, passage,
FTS, embedding, and Research state.

## D. Previous-release upgrade and local recovery

Prepare the reviewed plaintext V16/V2 profile:

```sh
pnpm release:acceptance prepare-upgrade upgrade-YYYYMMDD
pnpm release:acceptance launch upgrade-YYYYMMDD
```

Confirm the amber migration tidbit appears, exact search returns it, and its
URL citation opens. Quit normally:

```sh
pnpm release:acceptance check-upgrade upgrade-YYYYMMDD
```

The check requires current live heads plus a hashed, integrity-checked
pre-migration V16/V2 safety snapshot. Prove the recovery point without
modifying the upgraded source profile:

```sh
pnpm release:acceptance prove-upgrade-recovery \
  upgrade-YYYYMMDD \
  recovered-YYYYMMDD
pnpm release:acceptance launch recovered-YYYYMMDD
```

Inspect the restored tidbit/search/citation, quit, then run:

```sh
pnpm release:acceptance check-upgrade recovered-YYYYMMDD
```

This proves a verified pre-migration pair can populate a replacement profile,
migrate, and reopen. It is not an off-site backup claim.

## E. Installed application

After the isolated profile passes, install the exact candidate in
`/Applications` using `docs/RELEASE.md`. Repeat a short Quick Add, lexical
search/citation, normal quit, and reopen against the intended production
profile. No development tools are required on the installed Mac; only the
optional Claude CLI is external.

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
publication, restore tidbits/revisions/media/search/Research citations, and
show zero remote residue. The normal startup proof uses hidden app windows and
an isolated home.

## Release record

| Field                                       | Result  |
| ------------------------------------------- | ------- |
| Version / commit / app SHA-256              | pending |
| macOS / hardware / tester / date            | pending |
| Menu icon and global shortcuts              | pending |
| Clean Quick Add and rich capture            | pending |
| Exact/phrase search and URL citation        | pending |
| Claude/model-unavailable core behavior      | pending |
| Image OCR search and region citation        | pending |
| PDF search and page citation                | pending |
| Semantic setup progress and hybrid citation | pending |
| Grounded Research and saved answer          | pending |
| Normal quit, restart, and process cleanup   | pending |
| V16/V2 upgrade and snapshot verification    | pending |
| Separate-profile recovery proof             | pending |
| Packaged real-R2 clean-directory recovery   | pending |
| `/Applications` installed smoke             | pending |

Preserve each profile and failure log until the record is reviewed. A green
database check cannot substitute for an unobserved UI journey.
