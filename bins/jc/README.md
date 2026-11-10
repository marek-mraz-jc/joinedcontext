# jc

The one binary. Every module below is a Cargo feature; `jc` with all features is the whole platform on one host (target: under 2 GB without the AI embedder at 100 req/s). Each module also builds alone (`cargo hack clippy -p jc --each-feature`, the nightly `ci-full` lane). UI: HTML rendered by the server.
