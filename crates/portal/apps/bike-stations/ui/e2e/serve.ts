import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { answer } from "../src/fixtures/bikes";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
const NAME = "bike-stations";
const ORIGIN = "http://portal.test";
export const BASE = `${ORIGIN}/apps/${NAME}/`;
/** The moment the page is opened at, so a station's update time reads the same every run. */
export const NOW = new Date("2026-10-07T09:00:00Z");
// The App's own endpoint over the mobility space, and the helsinki project's shared bikes endpoint.
const CONFIG = {
  // Where the entity panel links a station (SDK-40); shown, never followed.
  portal: "https://portal.hel.fi/projects/helsinki-mobility",
  slug: "own7kq3zr5mx2vwt6nbc4pjd7fgs3mob",
  orgDomain: "hel.fi",
  space: "mobility",
  transport: "origin",
  appName: NAME,
  language: "en",
  endpoints: [
    { name: "app-bike-stations", slug: "own7kq3zr5mx2vwt6nbc4pjd7fgs3mob", space: "mobility", types: ["BikeHireDockingStation"] },
    { name: "helsinki-bikes", slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", space: "helsinki", types: [] },
  ],
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
  await page.clock.setFixedTime(NOW);

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) {
      served.outside.push(url.href);
      return route.abort();
    }
    // The entity panel reads one station fresh by its id (SDK-40).
    const one = /\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
    if (url.pathname.includes("/api/endpoint/") && one) {
      const entity = (answer("BikeHireDockingStation") as { id: string }[]).find((row) => row.id === decodeURIComponent(one[1]));
      return route.fulfill({
        status: entity ? 200 : 404,
        contentType: "application/ld+json",
        body: JSON.stringify(entity ?? { title: "Not Found", status: 404 }),
      });
    }
    if (url.pathname.includes("/api/endpoint/")) {
      const body = answer(url.searchParams.get("type"));
      return route.fulfill({ status: 200, contentType: "application/ld+json", body: JSON.stringify(body) });
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
