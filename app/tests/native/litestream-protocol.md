# Litestream protocol verification

The backup runtime is not enabled by this slice. These checks establish the
binary and exact-transaction contracts that later backup state, ownership,
checkpoint, and restore work is allowed to depend on.

## Pinned artifact

- Litestream: `v0.5.15`
- Architectures: arm64 and x86_64, assembled as one ad-hoc-signed universal
  executable
- Universal SHA-256:
  `c535829126d7bb8f3e8c2e7a4f9e3507c63dad1ed91815824aeabf9a5217760b`
- Universal bytes: `77508256`
- Minimum packaged macOS: 14.0
- Exact-TXID L0 retention: 720 hours

`scripts/stage-litestream-sidecar.sh` verifies the pinned official checksum
file before extracting either archive. It then verifies archive and slice
hashes, deployment targets, system-only dependencies, both executable
architectures, the universal hash, license, and signature before writing only
to the ignored release staging directory.

## Deterministic local proof

From `app/` after staging:

```sh
pnpm litestream:verify-local
```

The verifier uses a unique ignored data root and a file replica. It proves:

- the control socket is mode `0600` beneath a mode-`0700` runtime directory;
- nonblocking sync returns a local TXID that normalizes to 16-character
  lowercase hexadecimal;
- remote-wait sync confirms the replica;
- restoring the fenced TXID includes the inside-fence write and excludes the
  immediately following write;
- dry-run and full-integrity JSON contracts parse;
- the exact fence still restores after ordinary compaction;
- with accelerated L0 expiry, an interior compacted TXID stops being
  restorable after its exact L0 disappears, which makes `720h` retention a
  correctness requirement;
- graceful shutdown performs its final sync and leaves no child process.

The 2026-07-30 implementation run passed all of these checks against the pinned
universal binary. Machine-readable receipts remain beneath ignored
`app/.data/litestream-protocol/runs/`.

## Real R2 proof

Copy `app/.env.example` to ignored mode-`0600` `app/.env`, fill the
bucket-scoped credentials, stage Litestream, then run:

```sh
pnpm litestream:verify-r2
```

The endpoint is derived from the Cloudflare account ID and selected
jurisdiction; arbitrary endpoints are not accepted. Each run uses a random
path beneath the confined development prefix, repeats exact-fence,
full-integrity, post-compaction, and graceful-shutdown recovery against R2,
then removes that path and verifies zero remaining objects.

The 2026-07-30 implementation run passed against the private `kosh-local`
bucket and removed its unique prefix with zero residue. Receipts contain no
credential values and remain beneath ignored
`app/.data/litestream-r2-protocol/runs/`.

Neither verifier is part of ordinary app startup. Missing credentials,
Litestream, or network access cannot affect capture or lexical search.
