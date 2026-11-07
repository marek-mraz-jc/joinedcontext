# Server WASM example: the interface

The interface of the SDK's server WASM example (T-3341, ADR-N-044). Its server is a WebAssembly
component on the shared host, in `joinedcontext-platform` at `examples/apps/rust-wasm-server`,
and is answered at `/apps/{name}/api`. This folder is the static half: React 19 that lists, adds
and deletes notes, and attaches a file to each one.

The browser sends a file straight to the store, through the presigned URL the server hands out.
So the App's policy must let the page reach the store's public origin. The static host's
`connect-src` for a `kind: wasm` App names it (T-3343).

```sh
pnpm install && pnpm test && pnpm build
```
