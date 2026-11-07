import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { answer } from "../src/fixtures/praha";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
const NAME = "praha-mapa";
const ORIGIN = "http://portal.test";
export const BASE = `${ORIGIN}/apps/${NAME}/`;
/** The day the upcoming events are counted from. */
export const NOW = new Date("2026-10-06T09:00:00Z");
const CONFIG = {
  slug: "qb3wx7nc5ztk2pmyr6hjd4vae2",
  orgDomain: "praha.eu",
  space: "praha-mesto",
  transport: "origin",
  appName: NAME,
  language: "cs",
  // Where the entity panel links a place for editing (SDK-40).
  portal: "https://portal.praha.eu/projects/praha",
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
    if (url.pathname.includes("/api/endpoint/")) {
      // The entity panel reads one place by its id, and the reader's access document (SDK-40).
      const one = /\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
      if (one) {
        const id = decodeURIComponent(one[1]);
        const row = [...answer("PointOfInterest"), ...answer("WasteContainerIsle")].find((candidate) => (candidate as { id: string }).id === id);
        return route.fulfill({ status: row ? 200 : 404, contentType: "application/ld+json", body: JSON.stringify(row ?? { title: "Not Found" }) });
      }
      if (url.pathname.endsWith("/access")) {
        return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ permissions: [], prohibitions: [] }) });
      }
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
