# Complete off-site checkpoint protocol

Main migration V20 adds a durable content clock and an append-only checkpoint
state machine. V21 persists each checkpoint's exact media snapshot so even a
large library can be validated with bounded keyset pages instead of being
materialized in process memory, requires that STRICT table during startup
schema validation, and transactionally reclaims its child rows when the
checkpoint becomes `PUBLISHED` or `FAILED`. The checkpoint header retains its
aggregate evidence without checkpoint-by-media growth. Every recoverable
main-database mutation advances the clock in the same SQLite transaction.
Backup configuration and checkpoint bookkeeping are excluded, so publishing
a checkpoint cannot recursively schedule another one.

## Publication contract

Kosh publishes a checkpoint only after all of these ordered facts hold:

1. the sole SQLite writer opens an immediate transaction, revalidates the
   enabled backup lineage, captures the content revision, migration heads, and
   deduplicated retained source/preview hashes in relational storage, requires
   every captured hash to be `UPLOADED`, and commits the snapshot with its
   `PREPARED` row;
2. the writer stays occupied while a bounded local-only Litestream control
   request bound to the daemon's exact configuration revision, target,
   backup-set lineage, and replica epoch returns the exact TXID containing that
   row, preventing either a later transaction or a stale daemon generation from
   slipping through the fence;
3. a bounded remote Litestream sync reports a replica TXID greater than or
   equal to that fenced TXID;
4. Kosh keyset-pages the persisted snapshot, heads every captured immutable
   media hash, and verifies byte length, binary content type, SHA-256 metadata,
   object-format version, total count/bytes, and the ordered set digest in
   bounded parallel batches;
5. only then does Kosh create the versioned JSON manifest with `If-None-Match:
*`, read back the exact bytes in a separately authorized operation, strictly
   decode them, derive the same
   lineage-confined key, and compare every published fact; and
6. the local row advances to `PUBLISHED` with a monotonic publication sequence.

Every remote operation is serialized with backup configuration changes and
revalidates the exact enabled target, revision, backup-set lineage, and replica
epoch immediately before it starts. The final local publication transition
performs the same check in SQL. A target change can therefore leave at most an
unreferenced immutable manifest from an already completed PUT; it cannot
publish stale-lineage evidence locally or continue another remote operation.

The manifest is therefore the final commit record. A missing or corrupt media
object is requeued for upload and prevents publication. A pre-existing
manifest is accepted only when its readback bytes are identical; any other
payload is an immutable-object conflict. Interrupted `PREPARED`, `FENCED`, or
`REPLICATED` rows become `FAILED` at coordinator startup without replacing the
last successfully published checkpoint.

## Scheduling and control

The background coordinator remains inert when backup is disabled. A changed
content revision is checkpointed after 60 seconds without another mutation,
or after five minutes of continuous mutation. Failed automatic attempts wait
at least 30 seconds before retrying. A configuration revision change re-arms
the same schedule even when authored content is unchanged. The backend
`backup_now` command bypasses the debounce, releases permanently failed media
rows into the retry queue, wakes the media worker, and returns a bounded typed
result. Relational, media-upload, and complete-checkpoint status remain
separate and contain no credentials or raw remote errors.

The Litestream checkpoint handle references only the supervisor's current
daemon generation. Reload, crash, disable, and shutdown clear that control
before process cleanup. Local fence and remote replication requests have both
inner command deadlines and outer completion deadlines. Coordinator shutdown
signals cancellation between bounded remote operations and detaches any
already-running network request after a short join grace, so app exit does not
wait for an R2 timeout.

## Executable evidence

The native suite proves:

- the PREPARED row is visible before local sync while every later writer
  message remains blocked;
- phase transitions are monotonic, failures preserve the last publication, and
  publication sequence is correct even if the wall clock moves backwards;
- authored mutations advance the content clock while checkpoint bookkeeping
  does not;
- media references persist with the PREPARED row and keyset-page as 8/8/3 for
  a 19-object snapshot, with oversized page requests rejected and child rows
  reclaimed on terminal transitions while aggregate evidence remains;
- startup rejects a migrated database whose required V21 STRICT snapshot table
  is absent;
- startup classifies incomplete attempts as failed;
- quiet-time and maximum-delay scheduling boundaries are exact;
- a missing, behind, equal, and ahead replica TXID have the expected result;
- all media HEAD operations precede the immutable manifest PUT and exact GET;
- backup configuration changes wait for an in-flight remote operation, revoke
  subsequent operations, and prevent a stale revision from becoming published;
- local and remote checkpoint syncs reject a Litestream daemon from any other
  configuration revision or lineage;
- replacing a backup set does not reinterpret historical checkpoint facts
  through the new set's upload queue, while still scheduling a new checkpoint;
- identical manifest replay is idempotent while different bytes fail closed;
- missing media and manifest readback failures never publish;
- checkpoint controls close with their daemon generation and enforce the
  writer-fence deadline; and
- a manual request round-trips through the worker without touching Keychain or
  the network when no backup target is enabled.

Run the focused evidence from the repository root:

```sh
cargo test --locked --manifest-path app/src-tauri/Cargo.toml \
  --features test-support --lib checkpoint
```

The full native suite additionally proves migration compatibility, transactional
media enqueueing, supervised Litestream behavior, release contracts, and
headless startup.
