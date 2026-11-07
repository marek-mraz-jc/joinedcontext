import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import type { Inline } from "./App";
import type { Schema } from "./write";
import { parseSpec } from "./spec";
import "./index.css";

/**
 * The Portal inlines `{"slug": …, "spec": …, "data": …}` into `#kit-spec` when it serves the
 * preview document (Architecture/19 §1.2): the rows it read for the preview travel with the
 * document, because the sandboxed frame has no session to read them with. Without `data` the
 * kit reads the endpoint itself, which is the published app. `index.html` carries an example
 * for `vite dev`.
 */
const element = document.getElementById("kit-spec");
const root = createRoot(document.getElementById("root")!);
let payload: { slug?: unknown; spec?: unknown; data?: unknown; schema?: unknown; bridge?: unknown; basemap?: unknown } = {};
try {
  payload = JSON.parse(element?.textContent ?? "{}") as typeof payload;
} catch {
  payload = {};
}
const parsed = parseSpec(payload.spec);
const basemap =
  typeof payload.basemap === "string" && payload.basemap.trim() !== ""
    ? payload.basemap.trim()
    : undefined;
if (parsed.spec && typeof payload.slug === "string" && payload.slug !== "") {
  root.render(
    <StrictMode>
      <App
        slug={payload.slug}
        spec={parsed.spec}
        inline={typeof payload.data === "object" && payload.data !== null ? (payload.data as Inline) : undefined}
        schema={typeof payload.schema === "object" && payload.schema !== null ? (payload.schema as Schema) : undefined}
        bridge={payload.bridge === true}
        basemap={basemap}
      />
    </StrictMode>,
  );
} else {
  root.render(
    <div className="app">
      <header><h1>This dashboard cannot be shown</h1></header>
      <ul className="error-list">
        {(parsed.errors.length ? parsed.errors : ["slug: missing"]).map((error) => (
          <li key={error}>{error}</li>
        ))}
      </ul>
    </div>,
  );
}
