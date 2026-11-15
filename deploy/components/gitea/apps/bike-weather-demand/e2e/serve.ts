import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { ENTITIES, NOW, TEMPORAL } from "../src/fixtures/bikes";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "r7bxzgiatcruw4b7l2rrkufidz";
const CONFIG = {
  slug: SLUG,
  orgDomain: "hel.fi",
  space: "helsinki",
  transport: "origin",
  appName: "bike-weather-demand",
  // Where the entity panel links a station (SDK-40); shown, never followed.
  portal: "https://portal.hel.fi/projects/helsinki",
};

const TYPES: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".wasm": "application/wasm",
};

export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** The App's server (T-3355) as the page sees it: two kept models of whichever station is asked. */
const MODELS = {
  models: [
    { id: 2, trained_on: "2030-10-21", trained_at: "2030-10-21T06:00:00Z", hours: 160, enough: true, per_degree: 0.42, rain: -1.5, sigma: 2.25, weather_station: "urn:ngsi-ld:WeatherObserved:w1" },
    { id: 1, trained_on: "2030-10-20", trained_at: "2030-10-20T06:00:00Z", hours: 150, enough: true, per_degree: 0.4, rain: -1.2, sigma: 2.5, weather_station: "urn:ngsi-ld:WeatherObserved:w1" },
  ],
  stale: false,
};

/** Who the App's server says is signed in (T-3827): a viewer unless a test names a steward. */
export const VIEWER = { id: "u-1", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Sami Steward", roles: ["viewer", "steward"] };

/** Serves the built bundle at the root of the App's own host, with the SDK stub answering the endpoint. */
export async function serve(page: Page, entities = ENTITIES, temporal = TEMPORAL, me: { id: string; name: string; roles: string[] } = VIEWER): Promise<Served> {
  await page.clock.setFixedTime(new Date(NOW * 1000));
  const transport = stubTransport({
    entities,
    temporal,
    access: {
      permissions: [
        { resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" },
        { resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" },
      ],
      prohibitions: [],
    },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  // The App's server's inputs and history (T-3827), deciding by `me` as the real one does.
  const kept = new Map<string, Record<string, unknown>>();
  const history: unknown[] = [];
  const defaults = (station: string) => ({ station, weatherStation: null, radiusKm: 10, days: 7, note: "", custom: false, updatedBy: null, updatedAt: null });
  page.on("pageerror", (error) => served.problems.push(error.message));
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://portal.test") {
      served.outside.push(url.href);
      return route.abort();
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const body = route.request().postData();
      const answer = await transport({
        method: route.request().method() as "GET",
        path: url.pathname + url.search,
        body: body ? JSON.parse(body) : undefined,
      });
      return route.fulfill({
        status: answer.status,
        contentType: "application/json",
        body: JSON.stringify(answer.body ?? null),
      });
    }
    const api = "/apps/bike-weather-demand/api";
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname === `${api}/me`) return json(200, me);
    if (url.pathname === `${api}/history`) return json(200, history);
    const station = url.searchParams.get("station") ?? "";
    if (url.pathname === `${api}/inputs` && station.startsWith("urn:")) {
      if (method === "GET") return json(200, kept.get(station) ?? defaults(station));
      if (!me.roles.includes("steward")) return json(403, { detail: "this needs the role steward" });
      if (method === "PUT") {
        const set = { ...defaults(station), ...JSON.parse(route.request().postData() ?? "{}"), custom: true, updatedBy: me.name, updatedAt: "2030-10-21T06:00:00Z" };
        kept.set(station, set);
        history.unshift({ id: history.length + 1, at: "2030-10-21T06:00:00Z", actorName: me.name, action: "update", target: "inputs/station-1", summary: `${set.days} days` });
        return json(200, set);
      }
      kept.delete(station);
      return route.fulfill({ status: 204, body: "" });
    }
    if (url.pathname === "/apps/bike-weather-demand/api/models" && url.searchParams.get("station")?.startsWith("urn:")) {
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(MODELS) });
    }
    const snapshot = /^\/apps\/bike-weather-demand\/api\/models\/(\d+)\/snapshot$/.exec(url.pathname);
    if (snapshot) {
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ url: `http://portal.test/store/training/${snapshot[1]}.json` }) });
    }
    const file = normalize(url.pathname.slice(1) || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) {
      served.missing.push(url.pathname);
      return route.fulfill({ status: 404, body: "" });
    }
    let body = readFileSync(join(DIST, file));
    if (file === "index.html") {
      body = Buffer.from(
        body
          .toString("utf8")
          .replace(
            '<script id="jc-config" type="application/json"></script>',
            `<script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`,
          ),
      );
    }
    const headers =
      file === "index.html"
        ? { "Content-Security-Policy": "script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:" }
        : undefined;
    return route.fulfill({
      status: 200,
      contentType: TYPES[extname(file)] ?? "application/octet-stream",
      body,
      headers,
    });
  });
  return served;
}
