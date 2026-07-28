# Relevance fixtures

`v1.json` is the checked-in, versioned search-quality corpus. Each query names
graded relevant passage IDs, passages that must not rank, the retrieval mode it
is meant to exercise, and the exact citation locator expected from a result.
Only `text` and `searchMode` cross into a retriever. Categories, expected
passages, exclusions, and citation locators remain private to the scorer so the
system under test cannot read its answer key.

Run the fixture validator and intentionally empty baseline from `app/`:

```bash
pnpm relevance:validate
pnpm relevance:empty
pnpm relevance:lexical
```

The empty runner writes diffable JSON and text reports under the ignored
`app/.data/relevance/` directory. It succeeds as a command while the report
itself has `passed: false`, which lets future retrieval implementations reuse
the same report contract.

`relevance:lexical` writes a local copy of the report and should match the
checked-in `reports/lexical-v1.{json,txt}` baseline. The first baseline passes
16 of 17 queries, has Recall@10 0.9412, MRR 0.8971, nDCG@10 0.9067, exact and
phrase success 1.0, and zero forbidden hits. The remaining miss is the
misspelled concurrency query marked for combined lexical and semantic
retrieval; later ranking work can improve it without disguising the initial
baseline.

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
