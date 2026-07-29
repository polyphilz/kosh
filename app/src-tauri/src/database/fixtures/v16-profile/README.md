# V16 migration profile

This reviewed SQLite pair represents a real Kosh profile at main migration V16
and media migration V2. It contains one authored tidbit, immutable revision,
searchable passage, and URL-bearing source.

The normal Rust suite verifies both files against `manifest.json`, copies them
into a temporary profile, upgrades the pair with the embedded migrations, and
proves that exact search and citation resolution still target the authored
revision before and after a restart.

To deliberately regenerate the binary pair after reviewing a migration-boundary
change:

```sh
KOSH_REGENERATE_MIGRATION_FIXTURE=1 cargo test --locked \
  --manifest-path app/src-tauri/Cargo.toml \
  --features test-support \
  database::tests::regenerate_checked_in_v16_profile \
  -- --ignored --exact --nocapture
```

Update the two reviewed hashes in `manifest.json` only after inspecting the
fixture contents and proving the upgrade test passes.
