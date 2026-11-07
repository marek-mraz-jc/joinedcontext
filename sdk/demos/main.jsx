// The App templates' live demos (T-3306, AP-141): each sample over the template, with its own
// look, stylesheet and the rows of its `fixtures.ts`, served by the Portal at `/templates/{name}/`.
// No session, no token, no request leaves the page: the client is the SDK's stub.
import { createRoot } from "react-dom/client";
import { createElement as h } from "react";
import "../template/map-worker.ts";
import "../src/sdk/style.css";
import "../template/src/components/components.css";
import "../template/src/app.css";
import { applyTokens, JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

const SAMPLES = import.meta.glob("../samples/*/src/App.tsx");
const ROWS = import.meta.glob("../samples/*/src/fixtures.ts");
const CSS = import.meta.glob("../samples/*/src/app.css");
const TOKENS = import.meta.glob("../samples/*/src/design-tokens.json", { eager: true, import: "default" });
const ALL = { permissions: [{ actions: ["queryEntity", "retrieveEntity"], resource: { type: "*" }, attributes: "*" }], prohibitions: [] };

/** The template the address names: `/templates/{name}/`, a name the gallery holds or none. */
const name = /^\/templates\/([a-z0-9-]+)\/?/.exec(location.pathname)?.[1] ?? "";
const at = `../samples/${name}/src/`;
const root = createRoot(document.getElementById("root"));
if (!SAMPLES[at + "App.tsx"]) {
  root.render(h("p", null, "No such template."));
} else {
  const [{ default: Sample }, fixtures] = await Promise.all([SAMPLES[at + "App.tsx"](), ROWS[at + "fixtures.ts"](), CSS[at + "app.css"]?.()]);
  applyTokens(TOKENS[at + "design-tokens.json"]);
  const client = stubClient(
    { entities: fixtures.ROWS, schema: fixtures.SCHEMA, access: ALL, functions: fixtures.FUNCTIONS, temporal: fixtures.TEMPORAL, services: fixtures.SERVICES },
    { appName: name, endpointName: "demo", user: fixtures.USER ?? null },
  );
  document.title = `${name} · demo`;
  root.render(h(JcProvider, { client }, h(Sample)));
}
