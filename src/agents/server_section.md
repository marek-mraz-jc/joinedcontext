
## THE SERVER HALF (a wasm App)

This App has a server: `server/src/lib.rs`, a Rust crate compiled to a WebAssembly component on
the platform's shared host, answering `/apps/{appName}/api/*`, and its own Postgres table in
`migrations/0001_items.sql`. Change both to fit the request: rename the table and its columns,
add routes. The interface reaches it through `src/server.ts` (`useServer(useClient().config.appName)`),
never through `fetch` elsewhere.

What the crate may use, and nothing else (`server/Cargo.toml` and its lock are the Portal's):
- `jc_app_sdk::http::{Request, Response, Router, Params}`: `Router::new().get(path, f).post(path, f)`,
  a `{name}` segment is a param; `request.json::<T>()` returns `Result<T, Response>`;
  `Response::json(status, &value)`, `Response::problem(status, title, detail)`, `Response::no_content()`.
- `jc_app_sdk::sql::{query, execute, objects, rows, Value}`: bound parameters `$1, $2` only,
  `Value` from `i64`, `f64`, `bool`, `&str`, `String`, `Option<T>`; `Response::from_sql(err)`.
- `jc_app_sdk::gateway::get_json(path)`: the App's own Endpoint, `"/ngsi-ld/v1/entities?type=…"`.
- `jc_app_sdk::identity::caller()`: `Option<Caller { id, name, email, roles }>`, `None` for a visitor.
- `jc_app_sdk::email::send(&Email { to: Recipients::Me | Recipients::People(ids), subject, text, html })`
  and `jc_app_sdk::ai::complete(&Completion { messages: vec![Message { role: Role::User, content }],
  max_tokens, schema })` → `Answer { text, json, .. }`, only with `services: [email]` / `[ai]` in
  app.yaml; `Response::from_service(err)` answers a refusal.
- `serde` (derive, `#[serde(deny_unknown_fields)]` on every body) and `serde_json`.
- `pub fn handle(request: Request) -> Response` routed once, and `jc_app_sdk::app!(handle);`.

Never `std::net`, `std::env`, `std::fs`, `std::process`, `env!`, `include_str!`, a socket or a
path: the component has none and the build refuses the line (AP-147). No `unwrap()` or `panic!`
on a request: answer a problem. Keep the logic in plain functions and test them in
`#[cfg(test)] mod tests` in the same file: the build runs `cargo test` before it compiles the
component, and a failing test holds the build. A migration file is `migrations/NNNN_name.sql`,
`create table if not exists`, never dropping what an earlier one created.
