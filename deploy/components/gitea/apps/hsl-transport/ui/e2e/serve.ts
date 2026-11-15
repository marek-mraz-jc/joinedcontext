import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
// The App is the whole of its own host, `{name}.apps.{domain}`, and built with base `/` (AP-133).
const ORIGIN = "http://hsl-transport.apps.test";
export const BASE = `${ORIGIN}/`;
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };

/** The page's `JC_APP_CONFIG` as the reconciler hands it, without `user`; the Portal is shown, never followed. */
const CONFIG = { space: "helsinki", appName: "hsl-transport", portal: "https://portal.test/projects/helsinki" };

/** Six vehicles on four lines around the centre, in the shape the backend's snapshot answers. */
export const FLEET = [
  { id: "1", coordinates: [24.941, 60.171], bearing: 90, speed: 8.2, refLine: "550" },
  { id: "2", coordinates: [24.952, 60.168], bearing: 180, speed: 0, refLine: "550", mode: "bus" },
  { id: "3", coordinates: [24.93, 60.18], bearing: 45, speed: 11, refLine: "23", mode: "tram" },
  { id: "4", coordinates: [24.96, 60.19], bearing: 270, refLine: "71" },
  { id: "5", coordinates: [24.9, 60.2], bearing: 0, speed: 5, refLine: "1000N" },
  { id: "6", coordinates: [24.97, 60.16], bearing: 300 },
];

/** One stop's departures a few minutes after the run's own clock, and one of HSL's alerts (T-3895). */
const later = (minutes: number) => new Date(Date.now() + minutes * 60_000).toISOString();
export const DEPARTURES = [
  { id: "urn:ngsi-ld:GtfsStop:hel.fi:helsinki:2222", name: "Tapiola (M)", departures: [
    { line: "2550", headsign: "Itäkeskus via Pasila and a long way round", expected: later(3), status: "scheduled" },
    { line: "31M1", headsign: "Vuosaari", expected: later(6), status: "canceled" },
  ] },
];
export const ALERTS = [
  { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:hsl-1", title: "Tapiola: lifts out of order at the metro station entrance", severity: "medium",
    body: "Use the stairs or the escalators at the other end of the platform.", lines: ["2550"], stops: ["2222"],
    places: [{ stop: "2222", coordinates: [24.805, 60.175] }] },
];

/** What the served page did that it should not: a file it missed, an error it threw. */
export interface Served {
  missing: string[];
  problems: string[];
}

/**
 * Serves the built bundle at the root of the App's host the way the binary does, with its snapshot and
 * one frame of its stream answered from `FLEET`. Other hosts (the basemap's tiles) are refused.
 */
export async function serve(page: Page): Promise<Served> {
  const served: Served = { missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) return route.abort();
    const path = url.pathname.slice(1);
    if (path === "api/vehicles") return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(FLEET) });
    if (path === "api/departures") return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(DEPARTURES) });
    if (path === "api/alerts") return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(ALERTS) });
    if (path === "api/stream") {
      return route.fulfill({ status: 200, contentType: "text/event-stream", body: `event: vehicles\ndata: ${JSON.stringify(FLEET)}\n\n` });
    }
    const file = normalize(path || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) {
      served.missing.push(url.pathname);
      return route.fulfill({ status: 404, body: "" });
    }
    let body = readFileSync(join(DIST, file));
    // The backend writes JC_APP_CONFIG into its page; the entity panel links a bus from it (SDK-40).
    if (file === "index.html") {
      body = Buffer.from(body.toString("utf8").replace("<head>", `<head><script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`));
    }
    return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? "application/octet-stream", body });
  });
  return served;
}
