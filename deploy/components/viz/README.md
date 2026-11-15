# viz

The chart renderer of [ADR-N-053](https://github.com/marek-mraz/joinedcontext-docs/blob/main/Decisions/adr-n-053-declarative-visualizations.md)
on `https://viz.{domain}/` (T-4641). An Endpoint lists Vega-Lite 5 or Vega 5 specs in
`spec.catalog.visualizations`; the reconciler publishes each as a CKAN resource view of type
`jc_visualization` (T-4642); the catalogue frames this host to draw it.

Two documents, two origins of trust:

- **The host page** (`/#{viewId}`, `files/site/index.html` and `host.js`). Our code, no spec
  runs here. It reads the view anonymously from CKAN's action API (`resource_view_show` on
  `opendata.{domain}`, which answers CORS for this origin alone), checks the spec against the
  refusal list (`refuse.js`), fetches the declared source of that Endpoint on `api.{domain}`
  without a token (`csv`, `ogc`, `odata` or `temporal`), stops at 15 seconds, 50,000 rows or
  20 MB, and posts spec and rows to the renderer. `&inline` hides its title, description and
  link (the dataset page shows them); `&lang=sk` names the language. Any site may frame it: it
  holds no secret, and the "Embed on a website" snippet is exactly that frame.
- **The renderer** (`/render.html`, `render.js`). Framed by the host page with
  `sandbox="allow-scripts allow-downloads"`, so it runs in an opaque origin and reads no cookie
  of any host. Its CSP allows no connection (`connect-src 'none'`) and no `eval`; it checks the
  spec again and draws it with `vega-embed`, `ast: true` and `vega-interpreter`, with a loader
  that refuses every load, and offers PNG and SVG export only.

- **`#show`** (ADR-N-053 §2.9, T-4657). The host page opened as `/#show` reads no view: the
  assistant's widget posts one `{"type": "jc-viz-show", "spec", "rows", "lang"}`, accepted only
  from `https://{domain}` or `https://*.{domain}`. A Vega spec goes to the renderer as above; a
  map spec (`"$schema": "urn:joinedcontext:map:v1"`, GeoJSON and a popup field list) goes to
  **the map page** (`/map.html`, `map.js`): MapLibre on `tiles.{domain}` (T-4638), CSP
  `connect-src https://tiles.{domain}` alone, every popup value set as text, Enter opens the
  feature nearest the middle.

- **The explorer** (`/explore.html#{slug}&inline&lang=sk&type=T&q=…&sort=attr;desc`, `explore.js`,
  T-5059, T-5082). The catalogue's dataset and resource pages frame it for every Endpoint. Its table
  is the Portal's Endpoint "Data" view, the same React component (`EndpointDataView`), built from
  one Portal commit into `/explorer/explorer.js` and `explorer.css` by
  `scripts/viz-explorer-bundle.sh` (`images/viz/explorer/SHA256SUMS` names the commit; the image
  checks the sums). It reads `https://api.{domain}/api/endpoint/{slug}/` without a cookie or token:
  the schema's types and Fields (type and description per column), the grid's filters and search
  as NGSI-LD `q`, the order as `orderBy` (every row, on the server), paging with `count`, a full
  window and "Download these rows as CSV" (`file.csv` with the same `q` and `orderBy`). The Portal
  page shows the same view with the person's session. `explore.js` keeps the address (type, query
  and order in the hash, so a link opens the same view), a map of the same query as GeoJSON within
  the map's view (at most 1000 a view) and the Data API dialog with the query's NGSI-LD, OData, OGC
  and CSV addresses. Only the catalogue may frame it (`frame-ancestors https://opendata.{domain}`);
  it connects to the API and the tiles. A Portal change reaches the catalogue when the bundle is
  rebuilt from it and the image pinned (`images.yaml`).

- **Embedding on another site** (`/embed.html`, `embed.js`, T-5129). The explorer's **Embed** button
  gives a town the code for its own pages: the type, the kept filter (`q`) and order, table and map or
  table only, language, colours and height. Two forms, both the same page: an `<iframe>` of
  `/embed.html#{slug}&dataset=…&type=…&q=…&sort=…` (no script; it fits a WordPress "Custom HTML"
  block), or `<script src="https://viz.{domain}/embed.js">` with
  `<jc-data-explorer endpoint="…" dataset="…">`, which frames the same address and grows it to its
  content from the frame's own height messages. `/embed.html` is `/explore.html` with
  `frame-ancestors *` and `connect-src` adding the catalogue: it reads the dataset by name
  (`package_show`) and shows its title, publisher and licence above the table, only when that dataset
  names this very Endpoint. It is anonymous, sets and sends no cookie, and the public Endpoint checks
  `q` as for the catalogue. Both frames run sandboxed without `allow-forms`.
  `tests/embed.spec.mjs` runs the dialog to a town's page at 1440 and 400;
  `tests/wordpress/run.sh` pastes both codes into a real WordPress 6.8 page in Docker and runs
  `wordpress.spec.mjs` there.

The Vega 6.4.0, Vega-Lite 6.5.0, vega-embed 7.3.0, MapLibre 6.14.0 and pmtiles 4.5.0 builds are
the npm releases byte for byte (`images/viz/vendor/SHA256SUMS`), served from this host: no CDN.
vega-interpreter 2.3.2 ships only an ES module importing `vega-util`;
`scripts/viz-vega-interpreter.sh` writes the same code as a classic script over the `vega`
global. Vega-Lite 6 draws the Vega-Lite 5 specs the Endpoints publish. They are baked into the image
`ghcr.io/<owner>/joinedcontext-viz` (`images/viz/Dockerfile`, which checks the sums at build time;
built and signed by `.github/workflows/image-viz.yml`), because as chart files they took the Helm
release past the 1 MiB its Secret holds (T-4657). The image is pinned by digest in `images.yaml`.
MapLibre's two ES modules are stored gzipped, and nginx serves them as they are
(`gzip_static always`, `gunzip on`).

Tests: `tests/test_viz.py` (render, headers, checksums, and `node --test` on
`components/viz/tests/*.test.mjs`); the browser journey `components/viz/tests/journey.spec.mjs`
runs the good specs and the hostile ones with the hosts routed to these files, and
`explore.spec.mjs` runs the explorer over viewer, width, theme, language, data shape and failure,
a shared link's query and order, Fields, the full window and the Data API dialog, leaving a
screenshot of each state in `components/viz/tests/test-results/explore/`. It serves
`images/viz/explorer/`, so run `scripts/viz-explorer-bundle.sh` first after a Portal change.
