# Durable off-site media reconciliation

Main migration V19 adds a non-secret outbox for immutable media objects. Each
row is scoped to the current backup-set UUID and content SHA-256. Source blobs
and canonical image previews are seeded on enable, during a V18 upgrade with an
enabled configuration, and by the same SQLite transaction that creates a new
attachment, preview, authored revision membership, or retained research
evidence membership.

Changing the R2 destination resets the current set's upload evidence. Changing
backup sets removes stale local work and seeds the replacement namespace.
Disabling backup never blocks authored writes; re-enabling idempotently finds
references created while disabled.

## Worker contract

The single database writer performs only short reconciliation, claim, retry,
and completion transactions. A separate supervised thread:

1. leases one due digest with a fresh UUIDv7;
2. reads and hashes the bounded blob through a read-only media-database
   connection;
3. issues a hash-verified, create-only upload under the fixed R2 media key;
4. heads the object and verifies its byte length, content type, SHA-256
   metadata, object-format version, and remote version;
5. records `UPLOADED` only while the exact lease is still current.

Configuration saves and remote media operations share a process-local fence.
The worker revalidates its exact lease, enabled backup set, destination, and
current retained-reference predicate while holding that fence before PUT. An
orphaned running claim is deleted so a later reference can enqueue it afresh.
Therefore a disable, retarget, or final-reference removal either finishes first
and prevents the remote operation, or waits until an already started operation
has finished. Shutdown signals the worker, waits only a bounded grace period,
and detaches an uninterruptible HTTP request rather than stalling application
exit through the network timeout.

Network, authentication, rate-limit, and temporary local-read failures use a
bounded exponential retry. Missing or corrupt local content and conflicting
remote metadata fail closed. No media deletion capability is exposed. A crash
after R2 accepts the object but before SQLite records completion is safe:
startup recovers the lease, create-only replay observes the existing object,
and verified metadata completes the same digest idempotently.

## Executable evidence

The native suite proves:

- existing V18 references seed during migration;
- source and preview hashes enqueue transactionally, including rollback;
- enable/disable, target rotation, and backup-set replacement reconcile all
  retained hashes;
- stale lease holders cannot complete recovered work;
- retry timing and state survive a database close/reopen cycle;
- replay after a simulated post-upload crash is idempotent;
- remote metadata mismatch never records completion;
- disabling before a remote write fences the stale lease without touching R2;
- final-reference removal cancels the running row before remote write;
- disabling during a remote write waits for that operation's fence;
- an uninterruptible worker cannot extend shutdown beyond the grace period;
- a deliberately blocked object-store upload leaves the authored database
  writer responsive.

Run the focused evidence from the repository root:

```sh
cargo test --locked --manifest-path app/src-tauri/Cargo.toml \
  --features test-support --lib backup_media
cargo test --locked --manifest-path app/src-tauri/Cargo.toml \
  --features test-support --lib media_reconciler
```

Production startup treats coordinator creation and all remote failures as
optional degradation. With no enabled backup configuration, the worker claims
nothing and performs no Keychain or network operation.
