# Bento config unit-test suites

`bento/` is Bento v1.22.0's `config/test` tree (MIT, `LICENSE-bento`). YAML and proto files carry a
header naming their source; Bloblang and text files are verbatim, because a header would move the line
numbers their error texts name. `ekiben/` holds ekiben's own suites, one of which fails on purpose so
the failure report is compared too.

`expected/<suite>.out` is what `.bento/bento test <suite>` printed, run from the repository root,
followed by its exit code. `cargo test -p ekiben-server --test bento_config_tests` runs `ekiben test`
on every suite and compares byte for byte. `sh scripts/bento-config-tests.sh` records them again;
`BENTO_SRC=<bento v1.22.0 checkout>` also recopies `bento/`.

| Result | Suites |
|---|---|
| pass in both | 30 |
| fail in both, same report | 1 (`ekiben/failures.yaml`) |
| not applicable: `awk` processor | `bento/awk.yaml` |
| not applicable: Bloblang `cache_get`/`cache_set` | `bento/bloblang/cache_resource.yaml` |
| not applicable: `protobuf` processor | `bento/protobuf/house.yaml`, `bento/protobuf/people.yaml` |

The protobuf suites pass in Bento only from Bento's own repository root (their `import_paths` are
relative to the working directory), so their recorded output here is Bento's setup error.
