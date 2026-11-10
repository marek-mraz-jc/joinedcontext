# ekiben

A stream processor in Rust that runs Bento pipeline configs unchanged: Bloblang, processors, HTTP, file and NATS JetStream
connectors, streams API, one-shot (`ekiben run`) and scheduled (`ekiben schedule`) runs.

Goal: byte-identical output to Bento 1.22 on every pipeline in joinedcontext-deployment (`components/context-gateway/seed/**/*-bento.yaml`).
No Kafka: NATS JetStream is the only streaming backend.

Checks: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo audit`, `cargo deny check`.

End-to-end parity: `scripts/e2e-parity.sh` runs every deployment seed pipeline, rendered as the Portal renders it
(`tools/portal-render`), in Bento 1.22.0 and in `ekiben run` against a replay stub; the verdicts are in
`docs/parity/e2e-pipelines.md`. CI checks a frozen sample of 20 (`tests/e2e-parity`) without the Bento binary.

## Pages (a deviation from Bento)

A stream's processors run depth first, and a batch larger than `pipeline.page_rows` (default 1,000) goes on as pages
wherever every later step up to the next `split` reads each message on its own (`ekiben-core/src/paging.rs`):
`unarchive` (json_array, lines, csv, json_documents) makes each page's messages only when that page runs, and the
runner writes a page before the next is read, so a run holds a few pages, not its source. A step that joins the
batch (`archive`, `group_by*`, `while`, a mapping naming `batch_index()`, `batch_size()`, `from()` or `from_all()`)
keeps it whole. The rows written and their order are Bento's; a later `split` groups each page on its own, and a
run that fails on a page has written the pages before it (the run ends failed: `last_run.success` false).

## Decoders

`parquet_decode` (crate `ekiben-parquet`, arrow-rs parquet) and `protobuf` (crate `ekiben-protobuf`, protox +
prost-reflect) are cargo features of `ekiben-server`, on by default because seed pipelines use them; build with
`--no-default-features` for a smaller binary without them. The `protobuf` processor reads `import_paths` inside the
stream's file root when one is set; `bsr` (Buf Schema Registry) is refused at start.

Parity data: `crates/ekiben-{parquet,protobuf}/tests/parity/cases.json` holds each input with Bento 1.22.0's output.
`python3 scripts/make-parquet-fixtures.py` rewrites the synthetic Parquet fixtures, `python3 scripts/record-codec-parity.py`
re-records every case with `.bento/bento` (`sh scripts/reference-bento.sh` first).

Parity: `tests/parity/PARITY.md` lists every Bloblang text of the seed pipelines with its verdict against Bento 1.22.0,
and `crates/ekiben-server/tests/parity.rs` replays the recorded cases (`tests/parity/cases.json`, inputs and Bento's
stdout in `tests/parity/blobs/`). Regenerate with `sh scripts/reference-bento.sh`, then
`python3 scripts/parity/regen.py capture <joinedcontext-deployment clone> <dir>` (runs the deployment's Bento tests with
`scripts/parity/docker-shim.py` as `docker`, so each Bento run they make is recorded) and
`python3 scripts/parity/regen.py build <joinedcontext-deployment clone> <dir>`.
`ekiben run` writes its JSON summary to stderr; stdout belongs to the stream, as with `bento -c`.
