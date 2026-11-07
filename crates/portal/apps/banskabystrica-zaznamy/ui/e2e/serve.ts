import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { answer, CITY } from "../src/fixtures/records";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
const NAME = "banskabystrica-zaznamy";
const ORIGIN = "http://portal.test";
export const BASE = `${ORIGIN}/apps/${NAME}/`;
const CONFIG = {
  slug: "ovr4ttzywhad2oiogf67n7zyn2g2elfc",
  orgDomain: "banskabystrica.sk",
  space: "banskabystrica-mesto",
  transport: "origin",
  appName: NAME,
  language: "sk",
  // Where the entity panel links a record (SDK-40); shown, never followed.
  portal: "https://portal.example.org/projects/demo",
};
// What src/apps/static_host.rs sends for an embeddable app with no other origin to reach.
const CSP =
  "default-src 'self'; base-uri 'self'; object-src 'none'; script-src 'self'; " +
  "style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; " +
  "form-action 'self'; connect-src 'self'; frame-ancestors 'self'";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/**
 * Serves the built bundle under its published path the way the Portal static host does: the
 * `#jc-config` first in the head, the host's Content Security Policy, and the endpoint answered
 * with the fixture the component tests use.
 */
export async function serve(page: Page): Promise<Served> {
  const served: Served = { outside: [], missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) {
      served.outside.push(url.href);
      return route.abort();
    }
    // The city's grants (Policies app-banskabystrica-zaznamy-1 and -2): every member reads the rows
    // and writes the note, so the screen opens it to everyone (T-5137).
    if (url.pathname.includes("/api/endpoint/") && url.pathname.endsWith("/access")) {
      const permissions = [
        { resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
        { resource: { type: "KeyPerformanceIndicator" }, actions: ["updateAttrs"], attributes: ["stewardNote"] },
      ];
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ permissions, prohibitions: [] }) });
    }
    // The entity panel reads one record fresh by its id (SDK-40).
    const one = /\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
    if (url.pathname.includes("/api/endpoint/") && one) {
      const entity = answer(CITY).find((row) => row.id === decodeURIComponent(one[1]));
      return route.fulfill({
        status: entity ? 200 : 404,
        contentType: "application/ld+json",
        body: JSON.stringify(entity ?? { title: "Not Found", status: 404 }),
      });
    }
    if (url.pathname.includes("/api/endpoint/")) {
      // A chart's read names one cube (`q=dataSet=="…"`, T-2966); the table's page names none.
      const cube = /dataSet=="([^"]+)"/.exec(url.searchParams.get("q") ?? "")?.[1];
      const rows = answer(CITY).filter((entity) => !cube || (entity.dataSet as { value: string }).value === cube);
      return route.fulfill({
        status: 200,
        headers: { "content-type": "application/ld+json", "ngsild-results-count": String(rows.length) },
        body: JSON.stringify(rows),
      });
    }
    if (!url.pathname.startsWith(`/apps/${NAME}/`)) {
      served.missing.push(url.pathname);
      return route.fulfill({ status: 404, body: "" });
    }
    const file = normalize(url.pathname.slice(`/apps/${NAME}/`.length) || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) {
      served.missing.push(url.pathname);
      return route.fulfill({ status: 404, body: "" });
    }
    let body = readFileSync(join(DIST, file));
    if (file === "index.html") {
      body = Buffer.from(
        body.toString("utf8").replace("<head>", `<head><script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`),
      );
    }
    return route.fulfill({
      status: 200,
      headers: { "content-type": TYPES[extname(file)] ?? "application/octet-stream", "content-security-policy": CSP },
      body,
    });
  });
  return served;
}
