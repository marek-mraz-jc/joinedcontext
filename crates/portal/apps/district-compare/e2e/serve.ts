import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { ENTITIES } from "../src/fixtures/districts";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "2yawubrec4dv22ty3pyksixi7r";
const CONFIG = {
  slug: SLUG,
  orgDomain: "hel.fi",
  space: "helsinki",
  transport: "origin",
  appName: "district-compare",
  // Where the entity panel links a district (SDK-40); shown, never followed.
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

/** The App's server (T-3353) as the page sees it: two kept days of every district asked for. */
function history(codes: string[]) {
  const days = ["2030-10-21", "2030-10-20"].flatMap((day, n) =>
    codes.map((code, i) => ({ day, code, name: `District ${code}`, area_km2: 2, events: 3 - n + i, bikes: 1, bike_slots: 10, alerts: n, pm25: null, aqi: null })),
  );
  return { days, stale: false };
}

/** Serves the built bundle at the root of the App's own host, with the SDK stub answering the endpoint. */
/** Who the App's server says is signed in (T-3828): a viewer unless a test names a steward. */
export const VIEWER = { id: "u-1", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Sami Steward", roles: ["viewer", "steward"] };

export async function serve(page: Page, entities = ENTITIES, me: { id: string; name: string; roles: string[] } = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities,
    access: {
      permissions: [
        { resource: { type: "AdministrativeArea" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
        { resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
        { resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
        { resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
        { resource: { type: "AirQualityObserved" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
      ],
      prohibitions: [],
    },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  // The App's server's comparisons, pins and history (T-3828), deciding by `me` as the real one does.
  const kept: Record<string, unknown>[] = [];
  const pins: Record<string, unknown>[] = [];
  const changes: Record<string, unknown>[] = [];
  let next = 1;
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
    const api = "/apps/district-compare/api";
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const log = (action: string, summary: string) =>
      changes.unshift({ id: next++, at: "2030-10-21T06:00:00Z", actorName: me.name, action, target: "comparisons", summary });
    if (url.pathname === `${api}/me`) return json(200, me);
    if (url.pathname === `${api}/history`) return json(200, changes);
    if (url.pathname === `${api}/comparisons` && method === "GET") return json(200, kept);
    if (url.pathname === `${api}/comparisons` && method === "POST") {
      const id = next++;
      const made = { ...JSON.parse(route.request().postData() ?? "{}"), id, share: String(id).padStart(64, "0"), ownerName: me.name };
      kept.unshift(made);
      log("insert", `kept the comparison ${made.name}`);
      return json(201, made);
    }
    if (url.pathname === `${api}/default` && method === "GET") return json(200, pins[0] ?? null);
    if (url.pathname === `${api}/default`) {
      if (!me.roles.includes("steward")) return json(403, { detail: "this needs the role steward" });
      if (method === "PUT") {
        const made = { ...JSON.parse(route.request().postData() ?? "{}"), id: next++, ownerName: me.name };
        pins.unshift(made);
        log("insert", `pinned ${made.name} for everyone`);
        return json(200, made);
      }
      pins.shift();
      return route.fulfill({ status: 204, body: "" });
    }
    if (url.pathname === "/apps/district-compare/api/metrics") {
      const codes = (url.searchParams.get("codes") ?? "").split(",").filter(Boolean);
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(history(codes)) });
    }
    if (url.pathname === "/apps/district-compare/api/boundaries") {
      const files = { geojson: "http://portal.test/store/boundaries/districts.geojson", licence: "http://portal.test/store/boundaries/LICENCE.txt" };
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(files) });
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
