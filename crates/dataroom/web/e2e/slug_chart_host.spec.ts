import { readFileSync } from "node:fs";
import { extname, resolve } from "node:path";
import { test, expect, type Page, type Route } from "@playwright/test";

// A dataset page's chart in jc slug mode: Portal, catalogue (/opendata) and viz (/embed) on one
// host, the viz pages in a CSP sandbox (an opaque origin) as bins/jc/src/modules/viz.rs serves them
// beside the Portal (T-7464). jc-dev's barcelona-charging chart stayed blank: the chart host read
// its view from `opendata.{its host}`, a host that exists nowhere and that its own connect-src
// refused, and the edge's X-Frame-Options SAMEORIGIN refused /embed/render.html to the opaque host.
// No server is needed: every address is answered here with the headers jc and the edge send.
//   node <portal ui>/node_modules/@playwright/test/cli.js test -c crates/dataroom/web/e2e slug_chart_host
// SHOTS=<dir> also writes the 1440 and 400 shots.

const SITE = resolve(__dirname, "../../../../vendor/viz");
const JC = "https://jc.city.test";
const VIEW = "view-01a1daf9-0322-74dc-b9dc-5f48ccb01d5d";
const ENDPOINT = "hptanmkb42cdhkwvjminrm7mn4ejx4ah";
const DATASET = "barcelona-charging";

// viz.rs `Site::new` with base {JC}/embed, api {JC}, catalogue {JC}/opendata: shared, so sandboxed.
const SANDBOX = "sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads";
const TAIL = `base-uri 'none'; form-action 'none'; ${SANDBOX}`;
const CSP = {
  host: `default-src 'none'; script-src ${JC}; style-src ${JC}; img-src ${JC} data:; connect-src ${JC} ${JC}; frame-src ${JC}; ${TAIL}; frame-ancestors *`,
  render: `default-src 'none'; script-src ${JC}; style-src ${JC} 'unsafe-inline'; img-src ${JC} data:; connect-src 'none'; ${TAIL}`,
  // dataroom's page policy for a catalogue with no domain (csp_header_for_branding), nonce left out.
  catalogue: `default-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ${JC}; frame-src 'self' ${JC}; frame-ancestors 'self'; object-src 'none'; base-uri 'self'`,
};
const META = `<meta name="jc-viz" content="${JSON.stringify({ api: JC, tiles: "", catalogue: `${JC}/opendata` }).replace(/"/g, "&quot;")}">\n`;

const SPEC = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  width: "container",
  mark: { type: "bar", tooltip: true },
  encoding: {
    y: { field: "name\\.value", type: "nominal", title: "Station", sort: "-x" },
    x: { field: "availableCapacity\\.value", type: "quantitative", title: "Free connectors" },
  },
};
const VIEW_JSON = {
  success: true,
  result: {
    id: VIEW,
    resource_id: "res-pkg-01a1daf9-0294-7231-ada0-6253023f431c-0",
    package_id: DATASET,
    view_type: "jc_visualization",
    title: "Connectors lliures",
    description: "Free connectors per station.",
    endpoint: ENDPOINT,
    source: JSON.stringify({ source: "csv", type: "EVChargingStation" }),
    spec: JSON.stringify(SPEC),
  },
};
const CSV =
  "id,type,name.value,availableCapacity.value\n" +
  "urn:ngsi-ld:EVChargingStation:1,EVChargingStation,Sagrada Família,4\n" +
  "urn:ngsi-ld:EVChargingStation:2,EVChargingStation,Glòries,2\n" +
  "urn:ngsi-ld:EVChargingStation:3,EVChargingStation,Sants,6\n";

const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

// The catalogue frames the chart host as dataroom does beside the Portal (helpers.rs viz_frame):
// without allow-same-origin.
const DATASET_PAGE = `<!doctype html><html lang="sk"><head><meta charset="utf-8"><title>${DATASET}</title>
<style>body{font:16px system-ui;margin:0;padding:16px}iframe{width:100%;height:520px;border:0}</style></head>
<body><h1>Nabíjacie stanice Barcelona</h1>
<iframe title="Connectors lliures" src="${JC}/embed/#${VIEW}&amp;inline&amp;lang=sk" sandbox="allow-scripts allow-downloads"
 referrerpolicy="no-referrer"></iframe></body></html>`;

async function serve(page: Page, edgeOnRenderer: Record<string, string>) {
  await page.route("**/*", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== JC) return route.fulfill({ status: 404, body: "" });
    // jc's opaque-origin answers: CORS for `null` alone, never with credentials.
    const opaque = request.headers()["origin"] === "null" ? { "access-control-allow-origin": "null", vary: "Origin" } : {};
    if (url.pathname === "/opendata/api/3/action/resource_view_show" && url.searchParams.get("id") === VIEW) {
      return route.fulfill({ status: 200, headers: opaque, contentType: "application/json", body: JSON.stringify(VIEW_JSON) });
    }
    if (url.pathname === `/opendata/dataset/${DATASET}`) {
      const headers = { "content-security-policy": CSP.catalogue, "x-frame-options": "SAMEORIGIN" };
      return route.fulfill({ status: 200, headers, contentType: "text/html", body: DATASET_PAGE });
    }
    if (url.pathname === `/api/endpoint/${ENDPOINT}/file.csv`) {
      return route.fulfill({ status: 200, headers: opaque, contentType: "text/csv", body: CSV });
    }
    if (url.pathname.startsWith("/embed/")) {
      const name = url.pathname === "/embed/" ? "index.html" : url.pathname.slice("/embed/".length);
      if (!/^(vendor\/)?[a-z0-9.-]+$/.test(name)) return route.fulfill({ status: 404, body: "" });
      let body: Buffer | string = readFileSync(resolve(SITE, name.startsWith("vendor/") ? name : `site/${name}`));
      if (name === "index.html") body = body.toString().replace("<head>\n", `<head>\n${META}`);
      const csp = name === "render.html" ? CSP.render : name === "index.html" ? CSP.host : "default-src 'none'; frame-ancestors 'none'";
      const headers: Record<string, string> = { "content-security-policy": csp, ...opaque };
      if (name === "render.html") Object.assign(headers, edgeOnRenderer);
      return route.fulfill({ status: 200, headers, contentType: TYPES[extname(name)] ?? "application/octet-stream", body });
    }
    return route.fulfill({ status: 404, body: "" });
  });
}

const VIEWPORTS = [
  { width: 1440, height: 900 },
  { width: 400, height: 844 },
] as const;

for (const viewport of VIEWPORTS) {
  test(`a dataset page's chart draws beside the Portal with no console error (${viewport.width})`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(m.text());
    });
    await page.setViewportSize(viewport);
    // The edge's jc-embed plugin config: no X-Frame-Options on /embed (joinedcontext-deployment).
    await serve(page, {});
    await page.goto(`${JC}/opendata/dataset/${DATASET}`);

    const host = page.frameLocator(`iframe[title="Connectors lliures"]`);
    const chart = host.frameLocator("iframe").first();
    // One bar per station: the host read the view from the catalogue the meta names.
    await expect(chart.locator("g.mark-rect path")).toHaveCount(3, { timeout: 20_000 });
    await expect(host.locator("#status")).toBeHidden();
    expect(errors).toEqual([]);
    if (process.env.SHOTS) {
      await page.screenshot({ path: `${process.env.SHOTS}/slug-chart-${viewport.width}.png`, fullPage: true });
    }
  });
}

// Why the edge drops the header on /embed: the opaque chart host is no same origin to SAMEORIGIN.
test("the edge's X-Frame-Options SAMEORIGIN on the renderer blanks the chart", async ({ page }) => {
  const errors: string[] = [];
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  await serve(page, { "x-frame-options": "SAMEORIGIN" });
  await page.goto(`${JC}/opendata/dataset/${DATASET}`);
  await expect.poll(() => errors.some((e) => /X-Frame-Options/.test(e)), { timeout: 20_000 }).toBe(true);
  const host = page.frameLocator(`iframe[title="Connectors lliures"]`);
  await expect(host.frameLocator("iframe").first().locator("g.mark-rect path")).toHaveCount(0);
});
