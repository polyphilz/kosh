# Pre-redesign database pair

`main-v23.sqlite3` and `media-v2.sqlite3` are the closed, integrity-checked database pair created
by the deterministic redesign baseline seed on commit `0155ff2cec`. They intentionally preserve
the exact pre-redesign migration history and authored state so later code must migrate real old
bytes instead of recreating the fixture through newer write APIs.

The regression test pins both SHA-256 digests before opening the pair with the current binary.
