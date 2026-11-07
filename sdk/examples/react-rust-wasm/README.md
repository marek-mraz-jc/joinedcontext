# React + Rust/WebAssembly

The SDK's example of a `ui` App that computes in the browser (T-3327, AP-142). React on the SDK
fetches the entities, and a Rust function compiled to WebAssembly summarizes one number attribute
in a Web Worker, so the screen never waits on it. The static host serves it like any `ui` App:
no pod and no server memory per App.

| Path | What it is |
|---|---|
| `wasm/` | the crate: `summarize(values) -> [count, min, max, mean, median]`, with its own `Cargo.lock` |
| `src/summary.worker.ts` | loads `wasm/pkg` once and answers each question by its id |
| `src/App.tsx` | picks a type and a number attribute from the schema, asks the worker |
| `e2e/` | the bundle in Chromium under the static host's policy, and refused without `'wasm-unsafe-eval'` |
| `.gitea/workflows/build.yml` | the template's workflow on the `app-build-rust` runner |

Build it the way the lane does:

```sh
cd wasm && cargo test --locked && cargo build --release --target wasm32-unknown-unknown --locked
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/jc_wasm_example.wasm
cd .. && pnpm test && pnpm build && pnpm exec playwright test
```

`wasm-bindgen-cli` must be the version `wasm/Cargo.lock` names (0.2.129, the runner's). The lane
refuses another. Keep the crate without threads and its `.wasm` small: the lane checks nothing
about size, but every visitor downloads it.
