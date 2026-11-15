import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { STATIONS } from "../src/fixtures/stations";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "bikerebalancing";
// An operator of the project signed in; the panel links a station to the Portal (SDK-40), shown, never followed.
const CONFIG = {
  slug: SLUG,
  orgDomain: "hel.fi",
  space: "helsinki",
  transport: "origin",
  appName: "bike-rebalancing",
  language: "en",
  user: { id: "u-1", name: "Operaattori" },
  portal: "https://portal.hel.fi/projects/helsinki",
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

/** Who the App's server says is signed in (T-3826): a viewer unless a test names a steward. */
export const VIEWER = { id: "u-1", name: "Operaattori", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Ylläpitäjä", roles: ["viewer", "steward"] };

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, me: { id: string; name: string; roles: string[] } = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities: STATIONS,
    access: { permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  let levels = { low: 0.15, high: 0.85, target: 0.5 };
  const history: unknown[] = [];
  page.on("pageerror", (error) => served.problems.push(error.message));
  await page.clock.setFixedTime(new Date("2030-10-20T06:00:00Z"));

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://portal.test") {
      served.outside.push(url.href);
      return route.abort();
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const body = route.request().postData();
      const path = url.pathname + url.search;
      const answer = await transport({ method: route.request().method() as "GET", path, body: body ? JSON.parse(body) : undefined });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body ?? null) });
    }
    // The App's own server (T-3346, T-3826): an operator with no plan saved yet, the levels, and
    // the history, deciding by `me` as the real one does; any other call is missing.
    const api = `/apps/${CONFIG.appName}/api`;
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname === `${api}/plans` && method === "GET") return json(200, []);
    if (url.pathname === `${api}/me` && method === "GET") return json(200, me);
    if (url.pathname === `${api}/levels` && method === "GET") return json(200, levels);
    if (url.pathname === `${api}/levels` && method === "PUT") {
      if (!me.roles.includes("steward")) return json(403, { detail: "this needs the role steward" });
      levels = JSON.parse(route.request().postData() ?? "{}");
      history.unshift({ id: history.length + 1, at: "2030-10-20T06:00:00Z", actorName: me.name, action: "update", target: "levels/1", summary: "levels changed" });
      return json(200, levels);
    }
    if (url.pathname === `${api}/history` && method === "GET") return json(200, history);
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
