# New surface

A new read-only standard surface is a crate of its own, generated and wired in
by one command:

```sh
python3 dev/new-surface.py geojson --struct GeoJson \
  --title "GeoJSON of a view" \
  --description 'the Entities of one type of a view as an RFC 7946 `FeatureCollection`.'
```

The name is 2 to 16 of `a-z` and `0-9` and becomes the crate
`antares-NAME`, the broker feature `NAME` and the `ANTARES_API_SURFACES`
entry. The description is one sentence of Markdown (wrap code names in
backticks: the crate denies `clippy::doc_markdown`).

## 1. What it writes

| File | What |
|---|---|
| `crates/antares-NAME/Cargo.toml` | role `surface`, `antares-ops` the only workspace dependency, the full lint table (module rules M1, M2, 10.1) |
| `crates/antares-NAME/src/lib.rs` | `GET /x/views/{view}/NAME/v1/{type}` through `ViewOps::scan`: `limit`, `offset`, `q`, `scopeQ`, `lang`; read only; a hidden type, an unknown view and one without `query_entities` are `404`; `render` writes the page |
| `crates/antares-NAME/tests/NAME.rs` | seven tests that hold for any `render`: the view narrows, a hidden type is `404`, another Tenant is `400`, the Policy decides each read (`403`, no data), a write is `405` and stores nothing, paging and `q` narrow, bad parameters are `400` |
| `docs/src/NAME.md` | the book page, linked from the summary |
| `tests/NAME/README.md` | the conformance pack: the template's rows, then the standard's |

It also adds the crate to the workspace members and dependencies, a release
profile, the broker's optional dependency and feature, the `SURFACE_SHELF`
entry in `crates/antares-broker/src/main.rs`, the `ARCHITECTURE.md` row, the
row of the feature table in `docs/src/surfaces.md` and the vanilla-build gate
in `dev/check-no-addon.sh`. Each edit anchors on one
line; when an anchor has moved the script stops before writing anything.

## 2. What the author writes

1. `render(entities, next)` in `src/lib.rs`: the page in the standard's
   format, given the Entities in NGSI-LD normalized form and the next
   page's offset.
2. Tests of that format beside the generated ones, and their rows in
   `tests/NAME/README.md` with the clause each proves.
3. `cargo test -p antares-NAME`, `cargo clippy -p antares-NAME --all-targets
   -- -D warnings`. `dev/coverage-gate.py` (by hand) lists the
   lines and branches no test reaches; since T-6147 no gate requires 100 %.

`crates/antares-geojson` is the first surface made this way; the lines it
took are in `docs/analysis/coraine-architecture.md` section 3.1.

## 3. Outside this repository

The platform's gateway serves a surface only for an Endpoint that asks for
it: in joinedcontext-platform, `crates/context-gateway/src/handlers/ogc.rs`
gains a `Representation`, a `Surface` constant with the path `NAME/v1` and a
route, and the Endpoint manifest the representation name. That repository
has its own review, so the generator does not edit it.

## 4. Security

The generated crate holds no store handle: it reaches data through `Ops`
alone, so the view, the policy seam and row-level security decide every
read. `python3 dev/check-module-rules.py` refuses the crate if it names any
workspace crate but `antares-ops`, and `the_policy_decides_every_read` fails
if a read skips the policy seam. CI runs `dev/new-surface.test.sh`, which
generates a surface into a scratch copy of the tree and runs those checks.

## Related

- [Surfaces](surfaces.md)
- [Extending Antares](extending.md)
- [GeoJSON of a view](geojson.md)
