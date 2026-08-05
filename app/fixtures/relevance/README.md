# Relevance fixtures

`v1.json` is the checked-in search-quality corpus using fixture schema v3.
Every item is one stable current block owned by exactly one note. A block may
contribute authored text, attachment filenames, or image OCR. Files contribute
only their filename.

Each query declares graded block IDs, forbidden results, its retrieval need,
and one expected block. Only the query text and mode cross into a retriever;
the answer key remains private to the scorer.

Run the deterministic suite from `app/`:

```bash
pnpm relevance:validate
pnpm relevance:empty
pnpm relevance:lexical
pnpm relevance:hybrid
pnpm relevance:gate
```

The checked-in lexical and hybrid reports cover 25 queries. Both require full
Recall@10, exact expected-block resolution, exact/phrase success, and zero
forbidden hits. Hybrid MRR and nDCG@10 must remain at least 0.95. The manual
`block-audit-v1.json` sample verifies that ten expected blocks contain the
recorded searchable evidence.

Tests read `jina-v1-vectors.json` without downloading or starting a model.
Maintainers with the pinned model and sidecar can intentionally regenerate it:

```bash
KOSH_EMBEDDING_MODEL_PATH=/path/to/v5-nano-retrieval-Q8_0.gguf \
KOSH_LLAMA_SERVER_PATH=/path/to/llama-server \
cargo test --manifest-path src-tauri/Cargo.toml --test kosh-relevance -- \
  --kosh-relevance-cli hybrid-vectors
```

`pnpm relevance:scale` and `pnpm relevance:lexical-scale` exercise the
deterministic 10,000-note workload. The release-mode lexical benchmark runs 200
warmed production queries and enforces a 100 ms p95 interactive budget.
