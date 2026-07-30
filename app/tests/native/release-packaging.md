# Universal release packaging record

Status: **not yet recorded for the current release commit**.

The authoritative commands are:

```sh
cd app
pnpm release:build:app
pnpm release:smoke
```

Record one row only after both commands pass on the exact reviewed commit.

| Field                         | Result  |
| ----------------------------- | ------- |
| Kosh version / commit         | pending |
| Build Mac / macOS             | pending |
| App path                      | pending |
| App archive SHA-256           | pending |
| arm64 + x86_64 app slices     | pending |
| arm64 + x86_64 sidecar slices | pending |
| CPU + Metal fixture matrix    | pending |
| Sidecar SHA-256 / bytes       | pending |
| Ad-hoc deep signature         | pending |
| Identifier / minimum macOS    | pending |
| Exact resources and license   | pending |
| Model/test/secret exclusion   | pending |
| Fresh/restart packaged smoke  | pending |

Machine-readable checks own structure, hashes, signatures, migrations,
database health, and process startup. They do not claim that a person
successfully used the UI.
