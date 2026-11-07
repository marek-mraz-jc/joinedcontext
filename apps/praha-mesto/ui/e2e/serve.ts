import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import bikes from "../src/fixtures/bikes.json" with { type: "json" };
import parking from "../src/fixtures/parking.json" with { type: "json" };
import air from "../src/fixtures/air.json" with { type: "json" };

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
const NAME = "praha-mesto";
const ORIGIN = "http://portal.test";
export const BASE = `${ORIGIN}/apps/${NAME}/`;
const SLUG = "k3pq7vwyxz2a4b5c6d7e8f9g0h1j2m3n";
const CONFIG = {
  slug: SLUG,
  orgDomain: "praha.cz",
  space: "praha-mesto",
  transport: "origin",
  appName: NAME,
  language: "cs",
  endpoints: [{ name: "app-praha-mesto", slug: SLUG, space: "praha-mesto", types: ["BikeHireDockingStation", "OffStreetParking", "AirQualityObserved"] }],
  // Where the entity panel links an entity for editing (SDK-40).
  portal: "https://portal.praha.cz/projects/praha",
};
// What src/apps/static_host.rs sends for a public `ui` App with nothing else to reach.
const CSP =
  "default-src 'self'; base-uri 'self'; object-src 'none'; script-src 'self' 'wasm-unsafe-eval'; " +
  "style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; " +
  "form-action 'self'; connect-src 'self'; frame-ancestors 'self'";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };
const BY_TYPE: Record<string, unknown[]> = { BikeHireDockingStation: bikes, OffStreetParking: parking, AirQualityObserved: air };

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** Serves the built bundle under its published path as the Portal's static host does, the endpoint answered from the fixtures. */
export async function serve(page: Page): Promise<Served> {
  const served: Served = { outside: [], missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) {
      served.outside.push(url.href);
      return route.abort();
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const one = /\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
      if (one) {
        const id = decodeURIComponent(one[1]);
        const row = Object.values(BY_TYPE).flat().find((candidate) => (candidate as { id: string }).id === id);
        return route.fulfill({ status: row ? 200 : 404, contentType: "application/ld+json", body: JSON.stringify(row ?? { title: "Not Found" }) });
      }
      if (url.pathname.endsWith("/access")) {
        return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ permissions: [], prohibitions: [] }) });
      }
      const body = BY_TYPE[url.searchParams.get("type") ?? ""] ?? [];
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
      body = Buffer.from(body.toString("utf8").replace("<head>", `<head><script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`));
    }
    return route.fulfill({ status: 200, headers: { "content-type": TYPES[extname(file)] ?? "application/octet-stream", "content-security-policy": CSP }, body });
  });
  return served;
}
