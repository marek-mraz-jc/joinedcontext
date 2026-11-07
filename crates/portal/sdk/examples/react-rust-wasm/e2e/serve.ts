import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";

// The built bundle; the build lane points `dist/` at the bundle it just built (T-2827).
const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://app.test/";
const SLUG = "bikes";
const CONFIG = { slug: SLUG, orgDomain: "example.org", space: "demo", transport: "origin", appName: "wasm" };
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".woff2": "font/woff2" };

/** The policy the Portal's static host sends with every file of a `ui` App (AP-142). */
export const STATIC_HOST_CSP =
  "default-src 'self'; base-uri 'self'; object-src 'none'; script-src 'self' 'wasm-unsafe-eval'; " +
  "style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; form-action 'self'; " +
  "connect-src 'self'; frame-src 'self'; frame-ancestors 'none'";

const ROWS = [4, 10, 1, 7].map((bikes, i) => ({ id: `urn:ngsi-ld:BikeHireDockingStation:${i}`, type: "BikeHireDockingStation", name: `Station ${i}`, availableBikeNumber: bikes }));
const INDEX = { models: [{ version: 1, types: ["BikeHireDockingStation"] }] };
const SCHEMA = { definitions: { BikeHireDockingStation: { properties: { name: { type: "string" }, availableBikeNumber: { type: ["integer", "null"] } } } } };

/** Serves the bundle under `csp`, the stub answering the endpoint; returns the errors it threw. */
export async function serve(page: Page, csp = STATIC_HOST_CSP): Promise<string[]> {
  const transport = stubTransport({ entities: ROWS });
  const problems: string[] = [];
  page.on("pageerror", (error) => problems.push(error.message));
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== new URL(BASE).origin) return route.abort();
    const headers = { "Content-Security-Policy": csp };
    if (url.pathname === `/api/endpoint/${SLUG}/schema/index.json`) return route.fulfill({ headers, json: INDEX });
    if (url.pathname === `/api/endpoint/${SLUG}/schema/v1/json-schema`) return route.fulfill({ headers, json: SCHEMA });
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const answer = await transport({ method: "GET", path: url.pathname + url.search });
      return route.fulfill({ headers, status: answer.status, json: answer.body ?? null });
    }
    const file = normalize(url.pathname.slice(1) || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) return route.fulfill({ status: 404, body: "" });
    let body = readFileSync(join(DIST, file));
    if (file === "index.html") {
      body = Buffer.from(body.toString("utf8").replace('<script id="jc-config" type="application/json"></script>', `<script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`));
    }
    return route.fulfill({ status: 200, headers: { ...headers, "Content-Type": TYPES[extname(file)] ?? "application/octet-stream" }, body });
  });
  return problems;
}
