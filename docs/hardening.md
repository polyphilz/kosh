# Hardening and supported limits

Kosh treats titleless autosave and lexical search as the availability floor.
Optional embedding and extraction work may fail or be interrupted without
making authored notes or exact citations unavailable.

## Reproduce the hardening report

From the repository root on macOS:

```sh
scripts/run-hardening-report.sh
```

The command runs repository and secret policy, focused security contracts,
axe/keyboard/zoom/reduced-motion and light/dark DPR-1/DPR-2 browser suites, the
mixed native workload and restart test, the paired SQLite snapshot/restore
drill, the release-mode 10,000-tidbit lexical benchmark, and production bundle
inspection. It writes an ignored, commit-bound report to
`app/.data/hardening/report-v1.json`. A failed constituent command produces no
passing report.

The ordinary PR lanes run the same tests inside the complete frontend, browser,
native, relevance, bundle, and real-Tauri-startup jobs. The report command is a
local aggregation, not a substitute for any required CI job.

## Failure and recovery matrix

| Durable boundary | Forced/interrupted state exercised | Required recovery |
| --- | --- | --- |
| working copy and checkpoint | continuous typing, stale completion, navigation, failed quit, renderer interruption, restart | newest content remains recoverable; only exact generations checkpoint and a failed flush blocks quit |
| authored revision and FTS projection | concurrent create/edit/search/rebuild, shutdown, reopen | authored body, revision, source URL, exact passage ID, and citation remain stable |
| draft and attachment staging | interrupted reader, orphaned stage, committed/missing blob, concurrent ingestion | partial bytes removed; committed references never reaped |
| text/PDF/OCR extraction | pending, running, retry-wait, ready, failed, stale extractor, retired attachment | bounded batch requeue or terminal failure; stale output never becomes current |
| embedding index | dirty, running, partial vectors, model/version change | incomplete vectors requeued; index activates only when complete |
| migration and media reclamation | pending migration or irreversible eligible-media cleanup | verified main/media snapshot pair exists before mutation and reopens with integrity and citation provenance |

Startup media recovery may renew or retire lifecycle metadata and rebuild reap
candidates, but it never authorizes or deletes blob bytes. Reclamation occurs
only through explicit maintenance after its verified snapshot is published.

The exact tests are deliberately distributed beside the state machines:
`database/reliability_tests.rs`, `database/safety_snapshot.rs`,
`database/media_tests.rs`, `database/embedding_index_tests.rs`,
`database/working_copies.rs`, `notes/autosave.test.ts`, and the real-browser
note-route/editor suites.

## Resource and latency budgets

- The supported library target is 10,000 notes. Release lexical search must
  remain at or below 100 ms p95 for 200 deterministic queries through the
  WAL-backed production path.
- Visible cold process launch to a focused blank editor targets 1,000 ms p95.
  It is measured during the explicit macOS walkthrough because hidden startup
  automation cannot certify native visibility or focus without disrupting the
  active desktop. The automated report labels its native samples as hidden
  startup regression evidence only. Ordinary input must paint within one 60 Hz
  frame, and the warm Command-K overlay must open within 100 ms on the reference
  Mac. The report rejects shell regressions over 20% from the frozen
  pre-redesign measurement. BlockNote initialization has an explicitly
  reviewed 30% ceiling over the smaller ProseMirror editor it replaces;
  ordinary input retains the stricter frame budget. Already-running window
  reactivation is also measured visibly with a 150 ms p95 target because
  process restart is not an honest substitute.
- Both real WKWebViews must render and return startup/search/citation IPC within
  the 30-second native readiness ceiling on fresh and restarted profiles. This
  is a failure ceiling, not a desired launch time; the receipt retains exact
  commit and runtime evidence.
- The web production bundle is capped at 4,000,000 uncompressed bytes,
  2,700,000 JavaScript bytes total, and 1,100,000 bytes for any JavaScript
  chunk.
- The PDF worker is capped at 512 MiB address space and 32 MiB structured
  output. Semantic sidecar logs rotate at 5 MiB per file.
- Kosh retains at most three verified local safety-snapshot pairs. They are
  recovery points for migration/maintenance, not backup or multi-device sync.
  Before copying, Kosh rotates the oldest owned pair, computes a conservative
  main-plus-media copy budget with 64 MiB filesystem headroom, and checks free
  space. It may rotate additional old pairs when necessary but never deletes
  the newest recovery point merely to make room; insufficient storage then
  fails before either new database copy is allocated. Retention re-hashes both
  database files against the manifest, so a damaged newer pair cannot displace
  the newest still-valid recovery point.

Machine timings other than the 10k lexical budget are asserted only when the
CPU model, logical CPU count, and physical memory match the frozen reference
Mac; they are recorded without a pass/fail result on unlike developer hardware.
A regression is investigated from the ignored JSON report and may not be hidden
by widening a timeout.

## Attachment and extraction limits

| Input | Supported maximum |
| --- | --- |
| attachments per working copy | 32 |
| any direct attachment, image, or PDF | 32 MiB |
| display filename | 255 Unicode scalar values; no path separators, controls, colons, or bidirectional controls |
| searchable text extraction | first 4 MiB and at most 5,000 passages |
| PDF | 2,000 pages; OCR is attempted for at most 128 image-only pages |
| OCR result | 4,096 regions, 16,384 characters per region, 1,000,000 characters total |
| external temporary materializations | newest 16 attachments and newest 16 PDFs |

Archives and unknown/mismatched binaries are retained as opaque attachments.
Kosh never expands archives and never executes attachment content. A renamed
PDF is recognized from its header; a non-PDF with a PDF extension remains
opaque.

## Security and accessibility boundaries

Production CSP defaults to self-only execution and permits only Tauri IPC plus
typed `kosh-media:` image/object reads. Frames, forms, base retargeting, remote
connections, arbitrary filesystem/shell/process capabilities, and agent/web
research tools are absent. Source links are HTTP(S)-only. Local media access
requires a canonical UUIDv7 capability.

Every primary route is checked in light and dark mode with axe WCAG 2 A/AA and
2.1 A/AA rules. The suite also covers keyboard-only primary navigation, focus
restoration, screen-reader names, 200% text at the 720-pixel minimum window,
reduced motion, and pinned DPR-2 visual output. Native VoiceOver behavior and
installed-app rendering remain part of the packaged release checklist because
browser automation cannot truthfully certify them.
