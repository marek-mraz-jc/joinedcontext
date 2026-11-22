import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

// T-7504: the assistant's two charts (a count per decade, a daily average; the specs of
// crates/assistant chat/viz.rs) drawn by the renderer as bins/jc serves it: each in a frame of an
// opaque origin, under a CSP without unsafe-eval, the chart library fetched once for both.

const SITE = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../vendor/viz");
const TYPES: Record<string, string> = { ".css": "text/css", ".html": "text/html", ".js": "text/javascript" };
const csp = (own: string) =>
  `default-src 'none'; script-src ${own}; style-src ${own} 'unsafe-inline'; img-src ${own} data:; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors *`;

const BASE = { $schema: "https://vega.github.io/schema/vega-lite/v5.json", data: { name: "source" }, width: "container", autosize: { type: "fit", contains: "padding" } };
const DECADES = {
  spec: {
    ...BASE,
    mark: { type: "bar", tooltip: true },
    encoding: {
      x: { field: "label", type: "nominal", sort: null, title: null, axis: { labelAngle: -45, labelOverlap: true } },
      y: { field: "value", type: "quantitative", title: "Count" },
    },
  },
  rows: [{ label: "Before 1800", value: 4 }, { label: "1950–1959", value: 210 }, { label: "1960–1969", value: 298 }],
};
const DAILY = {
  spec: {
    ...BASE,
    mark: { type: "line", point: true, tooltip: true },
    encoding: {
      x: { field: "time", type: "temporal", title: null, scale: { type: "utc" } },
      y: { field: "value", type: "quantitative", title: "PM10 (µg/m³)", scale: { zero: false } },
      color: { field: "label", type: "nominal", title: null, legend: null },
    },
  },
  rows: Array.from({ length: 30 }, (_, i) => ({ label: "BB", time: `2026-10-${String(i + 1).padStart(2, "0")}`, value: 12 + (i % 7) })),
};

let server: Server;
let origin = "";
const hits = new Map<string, number>();

test.beforeAll(async () => {
  server = createServer((req, res) => {
    const name = (req.url ?? "").split("?")[0].slice(1);
    if (name === "parent") {
      res.writeHead(200, { "content-type": "text/html" });
      return res.end("<!doctype html><body></body>");
    }
    if (!/^(vendor\/)?[a-z0-9.-]+$/.test(name)) {
      res.writeHead(404);
      return res.end();
    }
    hits.set(name, (hits.get(name) ?? 0) + 1);
    const vendor = name.startsWith("vendor/");
    res.writeHead(200, {
      "content-type": TYPES[extname(name)] ?? "application/octet-stream",
      "cache-control": vendor ? "public, max-age=31536000, immutable" : "no-cache",
      ...(name === "render.html" ? { "content-security-policy": csp(origin) } : {}),
    });
    res.end(readFileSync(resolve(SITE, vendor ? name : `site/${name}`)));
  });
  await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

test.afterAll(async () => {
  await new Promise((closed) => server.close(closed));
});

test("two assistant charts load the bundle once and need no unsafe-eval", async ({ page }) => {
  const refused: string[] = [];
  page.on("console", (m) => {
    if (/Content Security Policy|unsafe-eval/i.test(m.text())) refused.push(m.text());
  });
  const policies: string[] = [];
  page.on("response", (r) => {
    if (r.url().endsWith("/render.html")) policies.push(r.headers()["content-security-policy"] ?? "");
  });
  await page.goto(`${origin}/parent`);
  for (const [id, chart, marks] of [
    ["decades", DECADES, "g.mark-rect path"],
    ["daily", DAILY, "g.mark-symbol path"],
  ] as const) {
    await page.evaluate(
      ([name, message]) => {
        const frame = document.createElement("iframe");
        frame.id = name;
        frame.setAttribute("sandbox", "allow-scripts allow-downloads");
        frame.style.cssText = "width:380px;height:320px";
        window.addEventListener("message", (event) => {
          if (event.source === frame.contentWindow && event.data.type === "jc-viz-ready") {
            frame.contentWindow?.postMessage(message, "*");
          }
        });
        frame.src = "/render.html";
        document.body.append(frame);
      },
      [id, { type: "jc-viz-render", spec: chart.spec, data: { format: "json", rows: chart.rows }, lang: "en", theme: "light" }] as const,
    );
    await expect(page.frameLocator(`#${id}`).locator(marks).first()).toBeVisible({ timeout: 20_000 });
  }
  const bundle = [...hits].filter(([name]) => name.startsWith("vendor/"));
  expect(bundle.length).toBeGreaterThan(0);
  expect(bundle.filter(([, n]) => n !== 1), "each library file is fetched once for two charts").toEqual([]);
  expect(policies.length).toBe(2);
  for (const policy of policies) expect(policy).not.toContain("unsafe-eval");
  expect(refused).toEqual([]);
});
