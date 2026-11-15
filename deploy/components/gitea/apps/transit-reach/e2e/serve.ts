import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import type { Row } from "@joinedcontext/sdk";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { HISTORY } from "../src/fixtures/vehicles";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "transitreach";
const CONFIG = { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "transit-reach", portal: "https://portal.test/projects/helsinki" };
// The static host's types (AP-142): a module the browser compiles must come as application/wasm.
const TYPES: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".wasm": "application/wasm",
};

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** Who the App's server says is signed in (T-3826): a viewer unless a test names a steward. */
export const VIEWER = { id: "u-1", name: "Vierailija", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Ylläpitäjä", roles: ["viewer", "steward"] };
type Person = { id: string; name: string; roles: string[] };

const BANDS = [
  { minutes: 10, areaKm2: 1.5, stops: 4 },
  { minutes: 20, areaKm2: 6.2, stops: 12 },
  { minutes: 30, areaKm2: 14, stops: 30 },
];

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, temporal: unknown[] = HISTORY, entities: Row[] = [], me: Person = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities,
    temporal: temporal as { id: string; type: string }[],
    access: {
      permissions: [
        { resource: { type: "Vehicle" }, actions: ["retrieveTemporal"], attributes: "*" },
        { resource: { type: "GtfsStop" }, actions: ["queryEntity"], attributes: "*" },
        { resource: { type: "PublicTransportRoute" }, actions: ["queryEntity"], attributes: "*" },
      ],
      prohibitions: [],
    },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  // One origin a steward published, one of the viewer's own; the history of what changes here.
  const origins = [
    { id: 2, name: "Pasila", lon: 24.9334, lat: 60.1986, source: "network", bands: BANDS, published: true, owner: STEWARD.id, createdAt: "2030-10-07T06:00:00Z" },
    { id: 1, name: "Kamppi", lon: 24.9314, lat: 60.1688, source: "network", bands: BANDS, published: false, owner: VIEWER.id, createdAt: "2030-10-07T05:00:00Z" },
  ];
  const history: unknown[] = [];
  page.on("pageerror", (error) => served.problems.push(error.message));
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://portal.test") {
      served.outside.push(url.href);
      return route.abort();
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const body = route.request().postData();
      const answer = await transport({ method: route.request().method() as "GET", path: url.pathname + url.search, body: body ? JSON.parse(body) : undefined });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body ?? null) });
    }
    // The App's own server (T-3826), deciding by `me` as the real one does; any other call is missing.
    const api = `/apps/${CONFIG.appName}/api`;
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const steward = me.roles.includes("steward");
    const shown = () =>
      origins.filter((o) => o.published || o.owner === me.id || steward).map(({ owner, ...o }) => ({ ...o, mine: owner === me.id }));
    if (url.pathname === `${api}/me` && method === "GET") return json(200, me);
    if (url.pathname === `${api}/origins` && method === "GET") return json(200, shown());
    if (url.pathname === `${api}/history` && method === "GET") return json(200, history);
    if (url.pathname === `${api}/origins` && method === "POST") {
      const made = { ...(JSON.parse(route.request().postData() ?? "{}") as { name: string }), id: origins.length + 1, published: false, owner: me.id, createdAt: "2030-10-07T07:00:00Z" };
      origins.unshift(made as (typeof origins)[number]);
      history.unshift({ id: history.length + 1, at: "2030-10-07T07:00:00Z", actorName: me.name, action: "insert", target: `origins/${made.id}`, summary: `kept the origin ${made.name}` });
      return json(201, { ...made, mine: true });
    }
    const published = url.pathname.match(new RegExp(`^${api}/origins/(\\d+)/published$`));
    if (published && method === "PUT") {
      if (!steward) return json(403, { detail: "this needs the role steward" });
      const found = origins.find((o) => o.id === Number(published[1]));
      if (!found) return json(404, { detail: "no such record, or not yours to change" });
      found.published = (JSON.parse(route.request().postData() ?? "{}") as { published: boolean }).published;
      history.unshift({ id: history.length + 1, at: "2030-10-07T07:00:00Z", actorName: me.name, action: "update", target: `origins/${found.id}`, summary: "published an origin" });
      return route.fulfill({ status: 204 });
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
    // The CSP the static host sends a `ui` App (AP-142): WebAssembly may compile, nothing evals.
    const headers = file === "index.html" ? { "Content-Security-Policy": "script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:" } : undefined;
    return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? "application/octet-stream", body, headers });
  });
  return served;
}
