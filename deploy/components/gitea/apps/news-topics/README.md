# Helsinki news topics

What the City of Helsinki's news talks about: topics, how their share changes week by week, and the articles behind each topic. One page, Topics:

- a ranked list of topics for the chosen period with their keywords and share;
- a stacked bar chart of topic share per ISO week (ECharts via ChartCard), with an accessible table alternative for screen readers;
- the articles behind the selected topic, with links to original sources; an article's title opens it in the SDK shell's entity panel, with a link to it in the Portal (the App writes nothing, so the panel offers no Edit);
- filters in the URL hash query (`#topics?weeks=8&topic=2&q=…`): period (4, 8, 12 weeks, or all), number of topics k (3–8, default 5), and title text search;
- interface in English, Finnish and Swedish (`?lang=fi|en|sv`), switched in the header.

Text analysis and clustering run client-side in WebAssembly (`news-topics-wasm` via Web Worker):
- language detection and tokenisation (English and Finnish stop words, simple stemming);
- TF-IDF sparse vector generation and L2 normalisation;
- cosine k-means clustering with deterministic k-means++ initialisation;
- keyword extraction from topic centroid weights;
- ISO week bucketing and weekly topic share calculation.

It is a `wasm` application on the joinedcontext App SDK (AP-148): React and Rust/WASM in the browser, read-only, one data need on Context Space `helsinki` (`Article`: `name`, `description`, `url`, `datePublished`, `source`). The Portal serves it under `/apps/news-topics/` and fills `#jc-config` with the endpoint configuration.

Its server component (`server/`, `wasm32-wasip2` on `jc-app-sdk`, T-3352) runs on the shared WASM host and keeps what the browser cannot: the feed holds only its recent articles, so the server reads them through the App's own Endpoint with the reader's token, fits the same topic model (`wasm/` without wasm-bindgen, five topics) to each ISO week, and keeps every week's topics and keywords in its tables `topic_runs` and `week_topics` (`migrations/`), recomputed when older than six hours. The week's articles go to the App's storage prefix as `corpus/<week>.json`; the page downloads them through a presigned URL.

| Route | What it does |
|---|---|
| `GET /apps/news-topics/api/weeks` | the topics of every kept week, the newest first |
| `GET /apps/news-topics/api/weeks/{week}/corpus` | a URL to download the week's articles from (`2026-W41`) |

## Prerequisites

Building the WebAssembly module requires the Rust `wasm32-unknown-unknown` target and `wasm-bindgen-cli`:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
```

## Roles and history

A signed-in reader follows topics of their own (T-3828), each by its top keyword, so a followed topic is found again in any period, at most 30; the topic list marks the followed ones and the Followed topics card says which are among the period's topics. A steward names a topic by its top keyword, renames it, and merges topics by giving them one name (the other topic's name, else its keywords); the page shows topics with one name as one, their shares summed. Everyone, signed in or not, reads the names. A viewer's naming controls stay closed with the reason as their tooltip. Every change shows at once and is taken back when the server refuses; the server records each in `app_audit`, and the Changes page lists a viewer's own and a steward's every one. Roles come from the caller's token through `identity::caller()`, never from the page.

| Route | Who | What it does |
|---|---|---|
| `GET /apps/news-topics/api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /apps/news-topics/api/follows` | signed in | the caller's followed keywords, newest first |
| `POST /apps/news-topics/api/follows` `{term}` | signed in | a keyword followed; 409 when followed already or past 30 |
| `DELETE /apps/news-topics/api/follows/{id}` | owner | no longer followed |
| `GET /apps/news-topics/api/names` | anyone | every topic name a steward gave |
| `POST /apps/news-topics/api/names` `{term, name}` | steward | the topic of that top keyword named, or renamed |
| `DELETE /apps/news-topics/api/names/{id}` | steward | the name taken back |
| `GET /apps/news-topics/api/history` | signed in | the newest 100 changes (a viewer: their own) |

The groups `news-topics-viewer` and `news-topics-steward` (`grants/users/groups/`) hold the demo viewer and steward. The App writes no context data, so its Endpoint's Policy stays read-only.

## Run the tests

```sh
(cd wasm && CARGO_TARGET_DIR=target cargo test --locked)   # the Rust functions, natively
pnpm install
pnpm wasm          # compiles wasm/ to WebAssembly and binds it into wasm/pkg/
pnpm test          # vitest, the module in wasm/pkg/ included
pnpm build         # the Vite bundle, the .wasm a hashed asset beside the worker
pnpm e2e           # Playwright at every width against the built bundle
```

## Run it locally against dev

`pnpm dev` serves the application on `http://localhost:5173/`. Fill the `#jc-config` element in `index.html` with the endpoint of the application on dev before starting:

```json
{ "slug": "<the endpoint slug the App page shows>", "orgDomain": "hel.fi", "space": "helsinki", "transport": "origin", "appName": "news-topics" }
```

Keep that edit out of the commit: the Portal writes the element when it serves the application.

`server/` is tested natively with `cargo test`, built with `cargo build --release --target wasm32-wasip2`, and played on the host against Postgres and RustFS from `server/host-test.json` by the platform's `crates/wasm-host/tests/apps_tests.rs`.

## Where it is built

This tree is the repository `helsinki_news-topics` on the installation's forge. Publishing a commit there builds it in the build lane (T-3327): `cargo test` and the WebAssembly build of `wasm/` against the lane's crate store, `wasm-bindgen --target web` into `wasm/pkg/`, then `pnpm test` and `pnpm build`.
