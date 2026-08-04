# Relevance fixtures

`v1.json` is the checked-in search-quality corpus using fixture schema v2.
Every passage declares whether it is authored or attachment-owned, and
attachment evidence carries a stable attachment identity plus its PDF page,
OCR region, or text-line locator. Each query names graded relevant passage IDs,
passages that must not rank, the retrieval mode it is meant to exercise, and
the exact citation locator expected from a result. Only `text` and `searchMode`
cross into a retriever. Categories, expected passages, exclusions, and
citation locators remain private to the scorer so the system under test cannot
read its answer key.

Run the fixture validator and intentionally empty baseline from `app/`:

```bash
pnpm relevance:validate
pnpm relevance:empty
pnpm relevance:lexical
pnpm relevance:hybrid
pnpm relevance:gate
```

The empty runner writes diffable JSON and text reports under the ignored
`app/.data/relevance/` directory. It succeeds as a command while the report
itself has `passed: false`, which lets future retrieval implementations reuse
the same report contract.

`relevance:lexical` writes a local copy of the report and should match the
checked-in `reports/lexical-v1.{json,txt}` baseline. The media-aware baseline
passes all 25 queries, has Recall@10 1.0, MRR 0.9650, nDCG@10 0.9684,
citation-locator accuracy 1.0, exact and phrase success 1.0, and zero forbidden
hits. OCR, PDF-page, text-line, pasted-URL, and misspelled concurrency queries
pass, while the authored passage ranks first in a PDF-volume stress query.

`relevance:hybrid` validates `jina-v1-vectors.json` against both the relevance
fixture digest and the shipped Jina v1 model hash, then writes a local report
that should match `reports/hybrid-v1.{json,txt}`. The vectors were generated
from the pinned model through Kosh's verified llama.cpp runtime; tests only read
the checked-in vectors and never download or start a model. The media-aware
hybrid report passes all 25 queries with Recall@10 and citation-locator accuracy
of 1.0, MRR 0.9657, nDCG@10 0.9691, exact/phrase success of 1.0, and zero
forbidden hits. Exact and code-identifier category metrics match the lexical
baseline.

`relevance:gate` is the release authority. It regenerates both reports in
memory, requires at least 25 queries, enforces explicit lexical and hybrid
metric floors, rejects precision regressions, and validates the ten-entry
manual citation sample in `citation-audit-v1.json`. The audit covers authored
and attachment evidence plus Markdown block, PDF page, OCR region, and text-line
locators. Its ignored JSON receipt can be retained by CI without treating
wall-clock observations as deterministic quality evidence.

Maintainers with the pinned model and sidecar can regenerate the vector fixture
before intentionally updating the reports:

```bash
KOSH_EMBEDDING_MODEL_PATH=/path/to/v5-nano-retrieval-Q8_0.gguf \
KOSH_LLAMA_SERVER_PATH=/path/to/llama-server \
cargo test --manifest-path src-tauri/Cargo.toml --test kosh-relevance -- \
  --kosh-relevance-cli hybrid-vectors
```

The generator rejects missing corpus/query coverage, non-normalized vectors,
fixture drift, and model-contract drift.

Generate the deterministic 10,000-tidbit workload and a separate runtime
metadata report with:

```bash
pnpm relevance:scale
pnpm relevance:lexical-scale
```

Set `KOSH_REFERENCE_HARDWARE` to a short machine label when producing a
reference performance report. Wall-clock measurements are observational until
a baseline is explicitly adopted; they are not brittle unit-test assertions.
The lexical benchmark measures 200 warmed queries over the 10,000-tidbit index
in release mode and fails its command when p95 exceeds the 100 ms interactive
budget.
