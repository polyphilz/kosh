# Research citation trust boundary

Kosh treats every byte produced by `claude -p` as untrusted. Partial text is
available only as an inert preview. The application does not make Markdown
links, citations, or navigation targets from streamed text.

## Issuing evidence handles

Each research run owns a read-only SQLite connection and a private, in-memory
citation registry. Kosh research tools return opaque handles shaped like
`cit_<40 lowercase hex characters>`. A handle identifies an immutable
`CitationResolution` captured when the tool response was built:

- exact passage and excerpt;
- revision and current/stale state;
- block, PDF page, OCR region, or text-line locator;
- tidbit or attachment identity;
- source labels and URLs stored by the user.

Database IDs, owner handles, and source URLs are not citation handles. Handles
are scoped to one process and disappear with its loopback MCP server.

## Grounding final output

The Claude prompt requires a citation beside every material claim using
`[[cite:cit_…]]`. Only the complete final result is parsed. Kosh resolves each
token against that run's registry and emits a `GROUNDED_FINAL_OUTPUT` event with:

- Markdown where valid tokens become numbered markers;
- byte ranges identifying the markers Kosh may make interactive;
- deduplicated citations containing their exact stored evidence;
- quality issues for unknown or malformed handles, citation syntax in code, and
  substantive paragraphs without a trusted citation.

An arbitrary marker that merely looks like `【1】` is not interactive because it
has no trusted mention range. Kosh resolves tokens only from parser-confirmed
plain Markdown text, outside links, images, raw HTML, code, and math. Invented
handles and malformed tokens in eligible prose become visibly unverified text
and never acquire a target.

Uncited-claim checks use rendered Markdown block boundaries. Adjacent list
items, table cells, and paragraphs are evaluated independently even when the
source has no blank line between them.

## Rendering contract

Consumers must render citations exclusively from `answer.citations` and
`answer.mentions`. Labels, excerpts, source links, attachment names, and
navigation targets come from the embedded `CitationResolution`, never from
Claude's text. The raw final CLI result is not emitted on the production path.

Source URLs are provenance supplied to Kosh by the user. They may be displayed
or opened from trusted citation detail, but Kosh Research does not fetch them
and has no web-search or web-fetch tool.

## Required adversarial coverage

Tests must continue to cover:

- invented and malformed handles;
- copied URLs and citation-looking plain text;
- repeated handles and exact evidence identity;
- stale revision snapshots;
- prompt injection embedded in tidbits;
- citation tokens inside code;
- uncited substantive claims;
- prompt wrapping and the live process event boundary.
