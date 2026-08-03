# Testing authorities

Kosh's detailed progressive test plan lives in the ignored implementation
workspace at `.plans/002-testing.md`. This tracked inventory identifies the
executable authorities that must remain green as product slices evolve. A test
may claim only the boundary it actually crosses.

## Required pull-request lanes

| CI lane | Command or runner | Authority |
| --- | --- | --- |
| repository policy | `scripts/check-repository.sh` | shell syntax, secret and ignored-data hygiene, negative tests for merge/runtime/bundle guards |
| frontend unit and type contracts | TypeScript, Oxlint, Oxfmt, Vitest | types, reducers, parsers, React state, typed Tauri protocol registry and Rust drift detection |
| browser functional and accessibility | Chromium Playwright plus axe | stateful capture, editing, search, citation, settings, focus and accessibility journeys |
| browser hardening | pinned DPR-2 Chromium Playwright plus axe | every primary route in light/dark, keyboard order, 200% reflow, reduced motion, high-DPI semantics |
| WebKit editor and keyboard contracts | WebKit Playwright | ProseMirror input, save, search selection and citation-focus behavior in Tauri's browser engine family |
| pinned visual contracts | single-worker Chromium Playwright | light/dark catalog, dialog, library and settings pixels at fixed viewports |
| production bundle isolation | `pnpm check:bundle` | no fake backend, fixtures, local data, model, database, test or environment material; explicit byte budgets |
| search and citation quality | `pnpm relevance:gate` and the 10k benchmark | pinned lexical/hybrid metrics, manual provenance sample, forbidden hits and interactive lexical latency |
| native unit and integration contracts | Rust tests and strict Clippy | migrations, writer serialization, files, workers, processes, search, citation and Tauri mock IPC |
| Litestream transaction protocol | `scripts/verify-litestream-ci-protocol.sh` | official native artifact pin, private control socket, exact writer fence, pre/post-compaction full-integrity restore, L0-expiry requirement and graceful no-orphan shutdown |
| R2 state, Keychain and object-store boundary | Rust fake tests plus the opt-in live probe in `app/tests/native/offsite-backup-foundation.md` | non-secret revisioned persistence, redacted versioned Keychain payloads, Cloudflare-only endpoints, fixed-prefix confinement, bounded conditional object operations and cleanup-after-failure |
| durable off-site media reconciliation | Rust queue/worker tests in `app/tests/native/offsite-backup-media.md` | transactional source/preview seeding, guarded leases, off-writer bounded reads, create-only upload verification, offline/restart replay and writer independence |
| supervised Litestream runtime | Rust supervisor/process tests in `app/tests/native/offsite-backup-litestream-runtime.md` | disabled inertness, private config/socket/PID ownership, bounded status, crash backoff, configuration reload, graceful final sync and local database independence |
| complete off-site recovery matrix | `pnpm backup:verify-fault-matrix` plus Rust restore/install tests | 64 named failures across snapshot, configuration, media, replication, checkpoint, discovery, restore, install and reopen; exact mapping to executable tests and non-destructive invariants |
| packaged real-R2 recovery | scheduled/manual `Packaged real-R2 recovery canary` workflow and `scripts/run-litestream-r2-canary.sh` | unique-prefix interrupted replication, manifest-last publication, drill, packaged clean-directory exact-TXID/media restore, normal hidden startup, search rebuild, authored citations and verified remote cleanup |
| native startup, restart, search and citation | `scripts/loop/runtime-gate.sh --ci` | real macOS Tauri process, both WKWebViews, fresh/restart persistence and actual runtime/search/citation IPC |
| universal release structure and smoke | `pnpm release:build:app && pnpm release:smoke` | icons/metadata/CSP/capabilities/entitlements, dual-architecture app and sidecar, signatures/resources, executable-to-source commit binding, packaged React/capture/search/citation IPC, fresh restart identity, no semantic model |
| packaged release journeys | `app/tests/native/release-acceptance.md` | clean installed capture/search, media extraction, hybrid retrieval, grounded citations, restart and separate-profile recovery |

The branch loop's native receipt must name the exact committed HEAD before a
PR can be merged.

## Determinism and isolation

- Browser tests use a new context and stateful fake backend per test. They fail
  on page errors, console errors, failed requests, or any external network
  request.
- Timed races use controlled promises and intercepted timers. Arbitrary sleeps,
  retries-to-green, test ordering, shared profiles and live network calls are
  forbidden.
- Visual baselines run separately with one worker, fixed locale, UTC timezone,
  viewport, fonts and appearance. Functional lanes do not own screenshots.
- Command, event and window names live in `src/tauriProtocol.ts`. A Vitest
  contract extracts Rust's production handler, emitted events and native
  window labels and requires exact equality.
- Native tests create explicit temporary roots. The runtime gate owns only
  `.kosh-loop`; it refuses unknown or symlinked profile paths and never resets a
  user's database.

## Search and citation evidence

The relevance suite has at least 25 realistic queries and checked model vectors.
The release gate requires:

- lexical Recall@10 and citation-locator accuracy at least 0.95;
- lexical exact/phrase success 1.0 and zero forbidden hits;
- hybrid Recall@10 and citation-locator accuracy 1.0;
- hybrid MRR and nDCG@10 at least 0.95;
- no hybrid regression against the lexical baseline;
- ten manually inspected citations spanning authored and attachment evidence,
  Markdown blocks, PDF pages, OCR regions and text lines;
- 10,000-tidbit lexical query p95 at or below 100 ms.

`scripts/check-bundle.sh` separately caps the uncompressed web bundle at
4,000,000 bytes, all JavaScript at 2,700,000 bytes, and any JavaScript chunk at
1,100,000 bytes. CI retains bounded JSON reports and failure traces for 7–14
days; it does not retain user data or credentials.

## Local full gate

From `app/`:

```sh
pnpm check
pnpm relevance:gate
pnpm relevance:lexical-scale
pnpm check:bundle
pnpm backup:verify-fault-matrix
```

After committing, aggregate the commit-bound hardening evidence from `app/`:

```sh
pnpm hardening:report
```

Then, from the repository root:

```sh
scripts/loop/runtime-gate.sh
```

Any changed commit invalidates that native receipt. Review and merge guards
then require green CI plus a clean Codex review of the same PR HEAD.
