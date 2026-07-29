# Hardening and supported limits

Kosh treats capture and lexical search as the availability floor. Optional
embedding, extraction, and Research work may fail or be interrupted without
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
| authored revision and FTS projection | concurrent create/edit/search/rebuild, shutdown, reopen | authored body, revision, source URL, exact passage ID, and citation remain stable |
| draft and attachment staging | interrupted reader, orphaned stage, committed/missing blob, concurrent ingestion | partial bytes removed; committed references never reaped |
| text/PDF/OCR extraction | pending, running, retry-wait, ready, failed, stale extractor, retired attachment | bounded batch requeue or terminal failure; stale output never becomes current |
| embedding index | dirty, running, partial vectors, model/version change | incomplete vectors requeued; index activates only when complete |
| Research run/process | queued, running, cancel, timeout, malformed/oversized stream, killed process | process group and workspace cleaned; durable run becomes interrupted and can be rerun |
| migration and media reclamation | pending migration or irreversible eligible-media cleanup | verified main/media snapshot pair exists before mutation and reopens with integrity and citation provenance |

The exact tests are deliberately distributed beside the state machines:
`database/reliability_tests.rs`, `database/safety_snapshot.rs`,
`database/media_tests.rs`, `database/embedding_index_tests.rs`,
`database/research_runs_tests.rs`, `claude.rs`, and `research/tests.rs`.

## Resource and latency budgets

- The supported library target is 10,000 tidbits. Release lexical search must
  remain at or below 100 ms p95 for 200 deterministic queries through the
  migrated WAL-backed production path.
- Both real WKWebViews must render and return startup/search/citation IPC within
  the 30-second native readiness ceiling on fresh and restarted profiles. This
  is a failure ceiling, not a desired launch time; the receipt retains exact
  commit and runtime evidence.
- The web production bundle is capped at 4,000,000 uncompressed bytes,
  2,700,000 JavaScript bytes total, and 1,100,000 bytes for any JavaScript
  chunk.
- The PDF worker is capped at 512 MiB address space and 32 MiB structured
  output. Semantic sidecar logs rotate at 5 MiB per file.
- Research prompts are capped at 64 KiB, parsed streams at 16 MiB and 16,384
  events, visible answer text at 1 MiB, and a run at two hours.
- Kosh retains at most three verified local safety-snapshot pairs. They are
  recovery points for migration/maintenance, not backup or multi-device sync.

Machine timings other than the 10k lexical budget are recorded rather than
asserted across unlike developer hardware. A regression is investigated from
the ignored JSON report and may not be hidden by widening a timeout.

## Attachment and extraction limits

| Input | Supported maximum |
| --- | --- |
| attachments per draft | 32 |
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
connections, arbitrary filesystem/shell/process capabilities, and web-enabled
Research tools are absent. Source links are HTTP(S)-only. Local media access
requires a canonical UUIDv7 capability, and untrusted Research text has Kosh
media capabilities neutralized before rendering.

Every primary route is checked in light and dark mode with axe WCAG 2 A/AA and
2.1 A/AA rules. The suite also covers keyboard-only primary navigation, focus
restoration, screen-reader names, 200% text at the 720-pixel minimum window,
reduced motion, and pinned DPR-2 visual output. Native VoiceOver behavior and
installed-app rendering remain part of the Chunk 28 release checklist because
browser automation cannot truthfully certify them.
