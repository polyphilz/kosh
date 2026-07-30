# Supervised Litestream runtime

Relational replication is optional background work. Production startup first
opens and recovers the local database, then starts a supervisor that loads only
an enabled non-secret backup configuration. With no enabled configuration it
does not resolve Litestream, read Keychain credentials, create runtime files,
or contact R2.

## Runtime boundary

For an enabled configuration the supervisor:

1. verifies the bundled universal Litestream bytes and release manifest;
2. creates `run/backup` with mode `0700`;
3. atomically writes a mode-`0600` fixed-protocol configuration containing
   environment-variable references rather than credential values;
4. loads the active backup-set credential from macOS Keychain into zeroizing
   process memory;
5. launches `litestream replicate` with an otherwise empty environment and a
   dedicated process group;
6. writes a bounded private PID record binding the exact executable, database,
   config, socket, backup set, replica epoch, and config digest;
7. accepts readiness only from a Unix socket with mode `0600`; and
8. confirms the canonical local and remote TXID through a bounded control
   command.

Status contains only fixed phase/error enums, canonical 16-character TXIDs,
one timestamp, and a saturating restart count. Raw child output and credential
material are never exposed or persisted. Remote health confirmation is killed
and reaped after two seconds so configuration changes and shutdown are not
stuck behind Litestream's full network timeout.

## Failure and shutdown contract

The supervisor polls for configuration changes and process exit independently
of the database writer. Transient launch, Keychain, process, and remote-sync
failures use exponential backoff capped at five minutes. A structural binary,
path, socket, PID-record, or configuration failure blocks only the current
configuration revision. Capture, editing, and Exact search continue through
all of these states.

On disable, retarget, application exit, or supervisor drop, Kosh sends SIGTERM
to the owned process group. The pinned Litestream configuration then performs
its graceful final remote sync for at most 30 seconds. Kosh allows a bounded
35-second process window, then kills and reaps the still-owned group. The PID
record and socket are removed only after their ownership checks pass. A stale
record is never used to kill an unrelated process, and an unowned socket is
never deleted.

## Executable evidence

The focused native suite proves:

- disabled configuration is completely inert;
- enabled configuration reaches `RUNNING` with equal canonical remote/local
  TXIDs;
- crashed children restart with capped backoff while database diagnostics
  remain responsive;
- transient failures recover and structural failures do not spin;
- disabling and service shutdown invoke exactly one graceful child shutdown;
- credential errors map to bounded redacted status;
- PID records are private and an owned dead runtime is swept; and
- an unowned socket fails closed without deletion.

Run it from the repository root:

```sh
cargo test --locked --manifest-path app/src-tauri/Cargo.toml \
  --features test-support --lib litestream_runtime
```

The existing Litestream protocol gate independently proves exact-TXID control,
the configured graceful final sync, process cleanup, and real-R2 behavior for
the pinned binary.
