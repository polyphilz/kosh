# Off-site backup state and R2 boundary

Relational replication is supervised only when backup is enabled. When backup
is absent or disabled, ordinary startup performs no network or Keychain
operation. Capture and Exact search therefore remain independent of backup
configuration.

## Persistence contract

Main migration V18 stores only non-secret, revision-guarded state:

- canonical UUIDv7 backup-set and replica-epoch IDs;
- enabled/disabled state;
- the fixed `R2` provider, Cloudflare jurisdiction, account ID, and bucket;
- created and updated timestamps.

There are deliberately no access-key, secret-key, endpoint, or prefix columns.
Changing backup sets queues the retired ID for durable Keychain cleanup.
Configuration writes use optimistic revisions and stale writes fail closed.

## Credential contract

R2 access and secret keys exist only in zeroizing process memory or a
versioned payload in the macOS Keychain service
`com.rohan.kosh.offsite-backup.r2`. Debug output is redacted. A Keychain save
is read back and decoded before it succeeds; a mismatched readback removes the
new item and fails.

## Network and namespace contract

The production client accepts only a validated Cloudflare account ID,
jurisdiction, and bucket. It derives one of Cloudflare's R2 S3 endpoints; no
caller-supplied URL is accepted. Object keys cannot be constructed freely.
They are derived beneath:

```text
kosh/v1/backup-sets/<canonical-backup-set-id>/
```

Both the production and fake clients reject keys or list prefixes belonging
to another backup set. Responses are bounded, redirects are disabled, HTTPS is
mandatory, returned keys are revalidated, and conditional writes are
supported.

## Probe contract

Unit tests run the complete put/head/get/list/delete probe through the fake,
including deterministic failure injection and cleanup-after-failure. To run
the same probe against the private development bucket from `app/`:

```sh
set -a
source .env
set +a
KOSH_RUN_R2_OBJECT_PROBE=1 cargo test --locked \
  --manifest-path src-tauri/Cargo.toml \
  --features test-support \
  live_r2_object_store_probe_uses_an_isolated_fixed_prefix_and_cleans_it
```

The live test uses a new backup-set ID, writes only one probe object, validates
its metadata and bytes, deletes it, and independently confirms the unique
probe prefix is empty. It never prints credential values. The 2026-07-30
implementation run passed against `kosh-local`.
