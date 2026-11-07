# kit — the dashboard the builder fills in

A React bundle built with the Portal and embedded in its binary. It renders one `spec.json`
(title, sources = entity types and attributes, filters, views: stats, map, table, chart, detail)
over the entities of one endpoint, read as `keyValues`, and nothing else. The Portal's kit pass
asks a model for the specification, applies it and serves this bundle inlined with it as the
preview document (Architecture/19 §1.2, AP-56…AP-60).

`pnpm install && pnpm dev` renders `spec.example.json` against `/api/endpoint/demo/…`, which is
what the demo server stubs. `pnpm test` runs the vitest suites; `pnpm build` writes `dist/kit.js`
and `dist/kit.css`, the two files `src/agents/kit.rs` embeds, and `dist/runtime/`: one ES module
per import name SDK-12 allows, their shared chunks and `runtime.json`, which
`src/agents/preview.rs` embeds as the `data:` modules of a code run's preview.
`JC_PREVIEW_OUT=… pnpm e2e` renders the template's preview document, written there by the
Portal's `agents::preview` test, in a sandboxed frame.
