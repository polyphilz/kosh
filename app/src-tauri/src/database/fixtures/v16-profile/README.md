# V16 migration profile

This reviewed plaintext SQL serialization represents a real Kosh profile at
main migration V16 and media migration V2. It contains one authored tidbit,
immutable revision, searchable passage, and URL-bearing source. No local
database file is committed.

The normal Rust suite verifies both SQL files against `manifest.json`,
materializes them into a temporary profile, upgrades the pair with the embedded
migrations, and proves that exact search and citation resolution still target
the authored revision before and after a restart. The serialized
`refinery_schema_history` rows retain the historical migration checksums, so
changing a shipped migration still fails the upgrade test.

To deliberately regenerate the serialization after reviewing a
migration-boundary change, create the V16/V2 pair in a temporary directory,
normalize its refinery timestamps, switch both files to DELETE journal mode,
vacuum them, and dump them with the system SQLite CLI:

```sh
sqlite3 temporary/kosh.sqlite3 .dump > main-v16.sql
sqlite3 temporary/media.sqlite3 .dump > media-v2.sql
```

Retain each original `PRAGMA application_id` in its SQL file. Update the two
reviewed hashes in `manifest.json` only after inspecting the plaintext fixture
and proving the upgrade test passes.
