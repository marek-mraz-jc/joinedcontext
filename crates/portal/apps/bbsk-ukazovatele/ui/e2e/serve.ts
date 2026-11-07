import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
const fixture = (name: string): string => readFileSync(fileURLToPath(new URL(`../src/fixtures/${name}`, import.meta.url)), "utf8");
const NAME = "bbsk-ukazovatele";
const ORIGIN = "http://portal.test";
export const BASE = `${ORIGIN}/apps/${NAME}/`;
const CONFIG = {
  // A member of the project signed in; the panel links an indicator to the Portal (SDK-40), shown, never followed.
  user: { id: "u-1", name: "Členka Projektu" },
  portal: "https://portal.bbsk.sk/projects/bbsk",
  slug: "7u4ns3cdg2mqlx5gmxhk7rqai6pmokdj",
  orgDomain: "bbsk.sk",
  space: "bbsk-kpi",
  transport: "origin",
  appName: NAME,
  language: "sk",
  endpoints: [
    { name: "bbsk-kpi", slug: "7u4ns3cdg2mqlx5gmxhk7rqai6pmokdj", space: "bbsk-kpi", types: ["KeyPerformanceIndicator"] },
    { name: "mesto-kpi", slug: "qfhhh5no5wz4lk3rfjisdtx3chiyfig3", space: "banskabystrica-kpi", types: ["KeyPerformanceIndicator"] },
    // The region's raw rows through bbsk-kraj's public endpoint, for the trend lines (T-2934).
    { name: "bbsk-kraj-verejne", slug: "krajverejne", space: "bbsk-kraj", types: ["KeyPerformanceIndicator"] },
  ],
};
/** What the gateway answers this App's reader on each endpoint: reading only (README). */
const ACCESS = JSON.stringify({ permissions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] });
/** What each body's endpoint answers: the fixtures the component tests read. */
const ANSWERS: Record<string, string> = {
  "7u4ns3cdg2mqlx5gmxhk7rqai6pmokdj": fixture("bbsk-kpi.json"),
  qfhhh5no5wz4lk3rfjisdtx3chiyfig3: fixture("banskabystrica-kpi.json"),
  // 532 rows: one page of the trend read.
  krajverejne: fixture("bbsk-kraj.json"),
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
    if (url.pathname.includes("/api/endpoint/") && url.pathname.endsWith("/access")) {
      return route.fulfill({ status: 200, contentType: "application/json", body: ACCESS });
    }
    // The entity panel reads one indicator fresh by its id (SDK-40).
    const one = /\/api\/endpoint\/([^/]+)\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
    if (one) {
      const entity = (JSON.parse(ANSWERS[one[1]] ?? "[]") as { id: string }[]).find((row) => row.id === decodeURIComponent(one[2]));
      return route.fulfill({
        status: entity ? 200 : 404,
        contentType: "application/ld+json",
        body: JSON.stringify(entity ?? { title: "Not Found", status: 404 }),
      });
    }
    if (url.pathname.includes("/api/endpoint/")) {
      const answer = ANSWERS[/\/api\/endpoint\/([^/]+)\//.exec(url.pathname)?.[1] ?? ""];
      if (answer === undefined) {
        served.missing.push(url.pathname);
        return route.fulfill({ status: 404, body: "" });
      }
      return route.fulfill({ status: 200, contentType: "application/ld+json", body: answer });
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
