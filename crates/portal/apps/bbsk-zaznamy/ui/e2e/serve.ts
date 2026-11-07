import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { answer, REGION } from "../src/fixtures/records";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
const NAME = "bbsk-zaznamy";
const ORIGIN = "http://portal.test";
export const BASE = `${ORIGIN}/apps/${NAME}/`;
const CONFIG = {
  slug: "7u4ns3cdg2mqlx5gmxhk7rqai6pmokdj",
  orgDomain: "bbsk.sk",
  space: "bbsk-kraj",
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
/** The region's grants (seed bbsk-app-zaznamy-policy*.yaml): every member reads, a steward also writes the note (T-5091). */
const READ = { resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" };
const NOTE_WRITE = { resource: { type: "KeyPerformanceIndicator" }, actions: ["updateAttrs"], attributes: ["stewardNote"] };

export async function serve(page: Page, role: "member" | "steward" = "steward"): Promise<Served> {
  const served: Served = { outside: [], missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) {
      served.outside.push(url.href);
      return route.abort();
    }
    if (url.pathname.includes("/api/endpoint/") && url.pathname.endsWith("/access")) {
      const permissions = role === "steward" ? [READ, NOTE_WRITE] : [READ];
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ permissions, prohibitions: [] }) });
    }
    // The entity panel reads one record fresh by its id (SDK-40).
    const one = /\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
    if (url.pathname.includes("/api/endpoint/") && one) {
      const entity = answer(REGION, "bbsk-kraj", "bbsk.sk").find((row) => row.id === decodeURIComponent(one[1]));
      return route.fulfill({
        status: entity ? 200 : 404,
        contentType: "application/ld+json",
        body: JSON.stringify(entity ?? { title: "Not Found", status: 404 }),
      });
    }
    if (url.pathname.includes("/api/endpoint/")) {
      return route.fulfill({
        status: 200,
        headers: { "content-type": "application/ld+json", "ngsild-results-count": String(REGION.length) },
        body: JSON.stringify(answer(REGION, "bbsk-kraj", "bbsk.sk")),
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
