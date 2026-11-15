import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { AIR_STATIONS, HISTORY, NOW, WEATHER_STATIONS } from "../src/fixtures/stations";
import { hourlyOf } from "../src/testing/hourly";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "airweatherexplorer";
const CONFIG = {
  slug: SLUG,
  orgDomain: "hel.fi",
  space: "helsinki",
  transport: "origin",
  appName: "air-weather-explorer",
  language: "en",
  // Where the entity panel links a station for editing (SDK-40); the link is shown, never followed.
  portal: "https://portal.test/projects/helsinki",
};
const TYPES: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
};
// What the Portal's static host sends for a `kind: ui` App once T-3327 allows WebAssembly: no
// 'unsafe-eval', only 'wasm-unsafe-eval', and nothing from another host.
const CSP = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'";

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** Who the App's server says is signed in (T-3827): a viewer unless a test names a steward. */
export const VIEWER = { id: "u-1", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Sami Steward", roles: ["viewer", "steward"] };

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, me: { id: string; name: string; roles: string[] } = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities: [...AIR_STATIONS, ...WEATHER_STATIONS],
    temporal: HISTORY,
    access: {
      permissions: ["AirQualityObserved", "WeatherObserved"].map((type) => ({ resource: { type }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const })),
      prohibitions: [],
    },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  const places: { id: number; name: string }[] = [];
  const faults: { id: number; station: string; note: string }[] = [];
  const history: unknown[] = [];
  page.on("pageerror", (error) => served.problems.push(error.message));
  await page.clock.setFixedTime(new Date(NOW));

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://portal.test") {
      served.outside.push(url.href);
      return route.abort();
    }
    // The App's server (T-3348): the stations' hourly means, as it keeps them.
    if (url.pathname === "/apps/air-weather-explorer/api/series") {
      const days = Number(url.searchParams.get("days") ?? "3");
      const from = Math.floor((NOW - days * 86_400_000) / 3_600_000) * 3_600_000;
      const air = hourlyOf(HISTORY, url.searchParams.get("air") ?? "", from);
      const weather = hourlyOf(HISTORY, url.searchParams.get("weather") ?? "", from);
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ air, weather }) });
    }
    // The roles (T-3827): the reader's places, a steward's marks and the history, deciding by `me`
    // as the real server does.
    const api = "/apps/air-weather-explorer/api";
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const sent = () => JSON.parse(route.request().postData() ?? "{}");
    const log = (action: string, target: string, summary: string) =>
      history.unshift({ id: history.length + 1, at: new Date(NOW).toISOString(), actorName: me.name, action, target, summary });
    const steward = me.roles.includes("steward");
    if (url.pathname === `${api}/me`) return json(200, me);
    if (url.pathname === `${api}/history`) return json(200, history);
    if (url.pathname === `${api}/places` && method === "GET") return json(200, places);
    if (url.pathname === `${api}/places` && method === "POST") {
      const place = { ...sent(), id: places.length + 1, latest: null, latestHour: null };
      places.unshift(place);
      log("insert", `places/${place.id}`, `kept the place ${place.name}`);
      return json(201, place);
    }
    if (url.pathname === `${api}/faults` && method === "GET") return json(200, faults.filter((f) => f.station === url.searchParams.get("station")));
    if (url.pathname === `${api}/faults` && method === "POST") {
      if (!steward) return json(403, { detail: "this needs the role steward" });
      const fault = { ...sent(), id: faults.length + 1 };
      faults.unshift(fault);
      log("insert", `faults/${fault.id}`, `marked faulty: ${fault.note}`);
      return json(201, fault);
    }
    if (url.pathname.startsWith(`${api}/`)) {
      served.missing.push(`${method} ${url.pathname}`);
      return json(404, { detail: "no such route" });
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const body = route.request().postData();
      const path = url.pathname + url.search;
      const answer = await transport({ method: route.request().method() as "GET", path, body: body ? JSON.parse(body) : undefined });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body ?? null) });
    }
    const file = normalize(url.pathname.slice(1) || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) {
      served.missing.push(url.pathname);
      return route.fulfill({ status: 404, body: "" });
    }
    let body = readFileSync(join(DIST, file));
    if (file === "index.html") {
      body = Buffer.from(
        body.toString("utf8").replace('<script id="jc-config" type="application/json"></script>', `<script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`),
      );
    }
    return route.fulfill({
      status: 200,
      contentType: TYPES[extname(file)] ?? "application/octet-stream",
      headers: { "content-security-policy": CSP },
      body,
    });
  });
  return served;
}
