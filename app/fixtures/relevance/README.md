# Relevance fixtures

`v1.json` is the checked-in, versioned search-quality corpus. Each query names
graded relevant passage IDs, passages that must not rank, the retrieval mode it
is meant to exercise, and the exact citation locator expected from a result.

Run the fixture validator and intentionally empty baseline from `app/`:

```bash
pnpm relevance:validate
pnpm relevance:empty
```

The empty runner writes diffable JSON and text reports under the ignored
`app/.data/relevance/` directory. It succeeds as a command while the report
itself has `passed: false`, which lets future retrieval implementations reuse
the same report contract.

Generate the deterministic 10,000-tidbit workload and a separate runtime
metadata report with:

```bash
pnpm relevance:scale
```

Set `KOSH_REFERENCE_HARDWARE` to a short machine label when producing a
reference performance report. Wall-clock measurements are observational until
a baseline is explicitly adopted; they are not brittle unit-test assertions.
