# Building a joinedcontext App: for AI agents

An App is a React page on `@joinedcontext/sdk`, with optional server functions
(`functions/*.ts`) and, in a `kind: wasm` App, a Rust server component on `jc-app-sdk`. The
platform runs and secures every service; the App only glues them together.

## What to import for which job

| Job | Import | Service |
|---|---|---|
| Read or write entities | `jc()`, `useEntities`, `useEntity`, `useSave`, `useSchema` | data |
| Who is signed in, what they may do | `useMe`, `useAccess`, `can` | identity |
| Tables, maps, charts, forms, the shell | the template's `src/components/` and the SDK's kit | none |
| Server-side logic in TypeScript | `functions/{name}.ts` with `@joinedcontext/sdk/server` | data |
| Server-side state in Rust | `server/src/lib.rs` with `jc_app_sdk::{http, sql, gateway, blob}` | data, files |
| Tests | `@joinedcontext/sdk/testing`: `stubClient`, `stubTransport`, `fakeContext` | none |

`services.json` lists every call of every service with its signature, the `app.yaml` lines it
needs and its quotas. `API.md` is the whole SDK reference.

## The five rules

1. Data goes through the App's Endpoint only: the SDK's client in the browser, `ctx.jc` in a
   function, `jc_app_sdk::gateway` in a component. Never `fetch` another host.
2. A service other than `identity` and `data` is listed in `spec.services`; an unlisted one
   refuses at run time with `layer: app` (SDK-45).
3. No key, token, password, host or tenant name in the App: the platform holds every credential.
4. Imports only from the list in `API.md`; a Rust component never names a socket, the
   environment, the file system or a process.
5. Every piece of logic ships with its test, and the tests pass before the App is published.
