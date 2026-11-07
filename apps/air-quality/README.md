# air-quality

The reference app of AP-34: the stations of one Context Space, their latest values, a day of
history, and a record form for a steward: add a station, correct its name, position and note,
remove a station a steward added. The measured values stay read-only. Architecture in
[Apps on Demand §6 and §13](https://github.com/marek-mraz-jc/joinedcontext-docs/blob/main/Architecture/16-apps-on-demand.md).

The roles come from the Portal's `GET {JC_ME_URL}`, asked with the edge's token (AP-109); they
decide what the page offers. What a person may write is decided by the gateway against the
App's per-role grants, and its refusal reaches the page word for word (AP-40).

The page sits in the SDK's `AppShell` (SDK-39). A station's card, its button beside the map and its
dot on the map open it in the SDK's entity panel (SDK-40), which reads and writes through this
App's backend (`ui/src/panel.ts`), never the gateway directly. In the panel a steward edits the
**steward note** and nothing else: the measured values are the station's, and the name per
language and the position stay with the record form. Anyone else reads the panel and, on the
App's own host, gets a link to the station in the Portal. The App's grants are unchanged: the
note was already a steward's `updateAttrs`.

It reaches exactly one thing, the Endpoint whose URL it is handed, and it holds no credential
of its own. The login in front of it is the platform edge (APISIX `openid-connect`), which
hands the user over as `X-Userinfo` and the user's access token as `X-Access-Token`; the app
carries that token to the endpoint and shows whatever comes back, refusal included. It has no
login, session or token code of its own, and a write without `X-Access-Token` is a 401 (AP-40).

## Run it

```bash
JC_ENDPOINT_URL=https://{host}/api/endpoint/{slug}/ \
JC_BASE_PATH=/apps/air-quality/ \
JC_ME_URL=https://{host}/api/v1/projects/{project}/apps/air-quality/me \
JC_BIND_ADDRESS=127.0.0.1:8080 \
cargo run -p air-quality
```

`JC_ENDPOINT_URL` is the only required variable (without `JC_ME_URL` nobody holds a role, so
nobody gets the form); in the pod the reconciler sets
`JC_BIND_ADDRESS=0.0.0.0:8080` and the readiness probe reads `/healthz`. Build the UI first with `pnpm install &&
pnpm build` in `ui/`; a debug build reads `ui/dist` from disk, a release build embeds it.

## Test it

```bash
cargo test -p air-quality                 # the backend and its contract with the endpoint
cd ui && pnpm test                        # the screens
cd ui && pnpm exec playwright test        # the steward and viewer flows against the binary
```

The browser flow starts the real binary against `tests/e2e/stub-endpoint.mjs` and plays the
edge itself, setting the `X-Access-Token` and `X-Userinfo` headers the plugin would set. Its
`widths` project (`ui/e2e/responsive.spec.ts`) holds the page, a reader's and a steward's, light and
dark, to 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, no control cut off, axe clean
(T-2825), and a station opened in the entity panel and its note corrected there at 375 and 1440 px,
light and dark (T-3374). The layout is the SDK's AppShell, Split, Card and Grid.

The App is under the Apps' coverage gate (T-3373): `sh ../../scripts/app-coverage-run.sh
air-quality` in a throwaway copy of `ui/`, and `cargo llvm-cov` here (95 % lines).
