import { readFileSync } from "node:fs";
import { extname, resolve } from "node:path";
import { test, expect, type Page, type Route } from "@playwright/test";

// A published Vega-Lite dashboard widget over a `csv` source, drawn by jc's chart renderer
// (vendor/viz/site) under the production Content Security Policy (T-6865). jc-prod's PM10
// dashboard showed "Evaluating a string as JavaScript violates ... 'unsafe-eval'" in place of its
// chart. No server is needed: every address is answered here, the pages with the headers jc-prod's
// viz host sends (curl of viz.smartcity.joinedcontext.com, 2026-11-14) on the domain city.test.
//   node <portal ui>/node_modules/@playwright/test/cli.js test -c crates/dataroom/web/e2e dashboard_viz_csp
// SHOTS=<dir> also writes the 1440 and 400 shots of each language.

const SITE = resolve(__dirname, "../../../../vendor/viz");
const VIZ = "https://viz.city.test";
const API = "https://api.city.test";
const DATA = "https://opendata.city.test";
const VIEW = "view-01a1d01f-8b11-778e-a9c3-05451e9c7987";
const ENDPOINT = "mluyob4nz52lok3ssk7pgn5vwt";

const TAIL = "base-uri 'none'; form-action 'none'";
const CSP = {
  host: `default-src 'none'; script-src ${VIZ}; style-src ${VIZ}; img-src ${VIZ} data:; connect-src ${API} ${DATA}; frame-src ${VIZ}; ${TAIL}; frame-ancestors *`,
  render: `default-src 'none'; script-src ${VIZ}; style-src ${VIZ} 'unsafe-inline'; img-src ${VIZ} data:; connect-src 'none'; ${TAIL}; frame-ancestors *`,
  catalogue: `default-src 'none'; style-src 'unsafe-inline'; frame-src ${VIZ}; ${TAIL}`,
};

// The view jc-prod's catalogue serves for the widget (resource_view_show), and its Endpoint's
// file.csv: header and first row as served, two more stations in the same shape.
const SPEC = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  title: "PM10 podľa stanice (µg/m³)",
  width: "container",
  mark: { type: "bar", tooltip: true },
  encoding: {
    y: { field: "name\\.languageMap\\.sk", type: "nominal", title: "Stanica", sort: "-x" },
    x: { field: "pm10\\.value", type: "quantitative", title: "PM10 (µg/m³)" },
    tooltip: [
      { field: "name\\.languageMap\\.sk", type: "nominal", title: "Stanica" },
      { field: "pm10\\.value", type: "quantitative", title: "PM10 (µg/m³)" },
      { field: "pm10\\.observedAt", type: "temporal", title: "Meranie", timeUnit: "yearmonthdatehoursminutes" },
    ],
  },
};
const VIEW_JSON = {
  success: true,
  result: {
    id: VIEW,
    resource_id: "res-pkg-01a1d01f-8a8d-72f9-81e4-b098e8ff9f64-0",
    package_id: "public-air",
    view_type: "jc_visualization",
    title: "PM10 podľa stanice (µg/m³)",
    description: "",
    endpoint: ENDPOINT,
    source: JSON.stringify({ source: "csv", type: "AirQualityObserved" }),
    spec: JSON.stringify(SPEC),
  },
};
const CSV =
  "id,type,dateObserved.value,location.value.type,location.value.coordinates[0],location.value.coordinates[1],name.languageMap.en,name.languageMap.sk,observedAt.value,pm10.value,pm10.unitCode,pm10.observedAt\n" +
  'urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:eea-SK0263A,AirQualityObserved,2026-11-14T07:00:00Z,Point,19.115268,48.733256,"Station SK0263A, urban background","Stanica SK0263A, mestské pozadie",,9.5909,GQ,2026-11-14T07:00:00Z\n' +
  'urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:eea-SK0004A,AirQualityObserved,2026-11-14T07:00:00Z,Point,19.155,48.7356,"Station SK0004A, traffic","Stanica SK0004A, doprava",,21.4,GQ,2026-11-14T07:00:00Z\n' +
  "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:eea-SK0118A,AirQualityObserved,2026-11-14T07:00:00Z,Point,19.1408,48.7417,Station SK0118A,Stanica SK0118A,,14,GQ,2026-11-14T07:00:00Z\n";

const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

function catalogue(lang: string): string {
  return `<!doctype html><html lang="${lang}"><head><meta charset="utf-8"><title>Dashboard</title>
<style>body{font:16px system-ui;margin:0;padding:16px}iframe{width:100%;height:520px;border:0}</style></head>
<body><h1>Air quality in Banská Bystrica — PM10 by station</h1>
<iframe title="PM10" src="${VIZ}/#${VIEW}&amp;inline&amp;lang=${lang}"></iframe></body></html>`;
}

async function serve(page: Page, lang: string) {
  await page.route("**/*", async (route: Route) => {
    const url = new URL(route.request().url());
    const cors = { "access-control-allow-origin": VIZ };
    if (url.origin === DATA && url.pathname.startsWith("/api/3/action/resource_view_show")) {
      return route.fulfill({ status: 200, headers: cors, contentType: "application/json", body: JSON.stringify(VIEW_JSON) });
    }
    if (url.origin === DATA) {
      return route.fulfill({ status: 200, headers: { "content-security-policy": CSP.catalogue }, contentType: "text/html", body: catalogue(lang) });
    }
    if (url.origin === API && url.pathname === `/api/endpoint/${ENDPOINT}/file.csv`) {
      return route.fulfill({ status: 200, headers: cors, contentType: "text/csv", body: CSV });
    }
    if (url.origin === VIZ) {
      // jc serves site/<name> at <name> and vendor/<name> at vendor/<name> (bins/jc/src/modules/viz.rs).
      const name = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
      if (!/^(vendor\/)?[a-z0-9.-]+$/.test(name)) return route.fulfill({ status: 404, body: "" });
      const file = resolve(SITE, name.startsWith("vendor/") ? name : `site/${name}`);
      const csp = name === "render.html" ? CSP.render : name === "index.html" ? CSP.host : "default-src 'none'; frame-ancestors 'none'";
      return route.fulfill({
        status: 200,
        headers: { "content-security-policy": csp },
        contentType: TYPES[extname(name)] ?? "application/octet-stream",
        body: readFileSync(file),
      });
    }
    return route.fulfill({ status: 404, body: "" });
  });
}

const VIEWPORTS = [
  { width: 1440, height: 900 },
  { width: 400, height: 844 },
] as const;

for (const lang of ["sk", "en"] as const) {
  for (const viewport of VIEWPORTS) {
    test(`a csv Vega-Lite widget draws under the production CSP (${lang}, ${viewport.width})`, async ({ page }) => {
      const refused: string[] = [];
      page.on("console", (m) => {
        if (/Content Security Policy|unsafe-eval/i.test(m.text())) refused.push(m.text());
      });
      await page.setViewportSize(viewport);
      await serve(page, lang);
      await page.goto(`${DATA}/${lang}/dashboards/banskabystrica-ovzdusie-pm10`);

      const host = page.frameLocator(`iframe[title="PM10"]`);
      const chart = host.frameLocator("iframe").first();
      // One bar per station: the chart drew from the CSV.
      await expect(chart.locator("g.mark-rect path")).toHaveCount(3, { timeout: 20_000 });
      await expect(host.locator("body")).not.toContainText("Content Security Policy");
      await expect(host.locator("body")).not.toContainText("unsafe-eval");
      expect(refused).toEqual([]);
      // The chart, its title included, stays inside its frame at every width.
      const overflow = await chart.locator("svg").first().evaluate((svg) => {
        const box = svg.getBoundingClientRect();
        return [...svg.querySelectorAll("text")].filter((t) => t.getBoundingClientRect().right > box.right + 1).map((t) => t.textContent);
      });
      expect(overflow).toEqual([]);
      if (process.env.SHOTS) {
        await page.screenshot({ path: `${process.env.SHOTS}/dashboard-csv-${lang}-${viewport.width}.png`, fullPage: true });
      }
    });
  }
}
