# Off-site restore, takeover, and drill contract

Chunk 29f adds an offline disaster-recovery backend without changing local
capture or Exact search startup behavior.

The native test suite proves:

- checkpoint discovery is confined to the selected backup-set prefix, reads
  every page with hard bounds, accepts only canonical immutable manifests, and
  orders complete checkpoints newest first;
- restore preview uses Litestream's dry-run JSON for the manifest's exact
  16-character TXID and reports the bounded file and byte totals without
  changing the live library;
- the production restore command requests full integrity, validates the exact
  TXID and destination, clears inherited environment state, and supplies R2
  credentials only through the child's stdin credential descriptor;
- media reconstruction derives the retained source/preview hashes from the
  restored main database, downloads only confined content-addressed objects,
  verifies metadata, length, and SHA-256, and matches the manifest's ordered
  count, byte total, and hash-set digest;
- the rebuilt pair must have current checksummed migration heads, application
  IDs, full SQLite integrity, foreign-key integrity, exact media relationships,
  content bytes, search documents, and citation provenance;
- the offline two-file installer accepts only a newly reserved directory,
  publishes both files through descriptor-bound operations, and writes a
  durable completion receipt for idempotent retry;
- a complete clean-directory recovery reopens through normal `Database`
  initialization and restores Exact search, a source-bearing tidbit citation,
  and attachment bytes;
- a drill reconstructs and validates the same pair in an owned temporary
  directory, removes it afterward, and leaves the live database bytes
  unchanged;
- takeover compares the previewed remote owner and object version, requires a
  fresh replica epoch, uses one conditional replacement, and fails closed if
  another writer changed ownership first.

Run the focused backend proof from the repository root:

```sh
cargo test --locked --manifest-path app/src-tauri/Cargo.toml \
  --features test-support --lib backup::restore::tests
cargo test --locked --manifest-path app/src-tauri/Cargo.toml \
  --features test-support --lib backup::owner::tests
```

The ordinary full `pnpm check` remains the pull-request gate. The deterministic
48-case matrix is checked with:

```sh
pnpm --dir app backup:verify-fault-matrix
```

Real-R2 destructive recovery stays outside pull requests and is recorded by
the scheduled/manual packaged canary:

```sh
cd app
pnpm release:build:app
KOSH_R2_CANARY_REQUIRE_PACKAGED=1 ../scripts/run-litestream-r2-canary.sh
```

The canary uses a unique backup set, retries interrupted replication, drills
the published point, executes the package's `recovery remote-restore` command
into a brand-new isolated data directory, then starts the restored app with
hidden windows. Its redacted receipt proves tidbits, immutable revisions,
media bytes, rebuilt search, URL citations, historical note citations, and
zero remaining objects under the canary prefix.
