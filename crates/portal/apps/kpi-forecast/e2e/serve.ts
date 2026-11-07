import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { HISTORY, KPIS } from "../src/fixtures/kpis";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "kpiforecast";
// `portal`: where the entity panel links an indicator for editing (SDK-40); shown, never followed.
const CONFIG = { slug: SLUG, orgDomain: "hel.fi", space: "helsinki-kpi", transport: "origin", appName: "kpi-forecast", portal: "https://portal.test/projects/helsinki" };
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

/** Who the App's server says is signed in (T-3827): a viewer unless a test names a steward. */
export const VIEWER = { id: "u-1", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Sami Steward", roles: ["viewer", "steward"] };

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, entities = KPIS, temporal = HISTORY, me: { id: string; name: string; roles: string[] } = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities,
    temporal,
    access: {
      permissions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" }],
      prohibitions: [],
    },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  const published: { id: number; kpi: string; days: number; horizon: number; note: string; step: number; points: unknown[]; publisherName: string; publishedAt: string }[] = [];
  const follows: { id: number; kpi: string }[] = [];
  const history: unknown[] = [];
  page.on("pageerror", (error) => served.problems.push(error.message));
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://portal.test") {
      served.outside.push(url.href);
      return route.abort();
    }
    // The App's server (T-3350): today's forecasts already recorded, none kept for an indicator yet.
    if (url.pathname === "/apps/kpi-forecast/api/forecasts") {
      const body = route.request().method() === "POST" ? { recorded: 0, already: true } : [];
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    }
    // The roles (T-3827): a steward's published forecasts, the reader's follows and the history,
    // deciding by `me` as the real server does.
    const api = "/apps/kpi-forecast/api";
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const sent = () => JSON.parse(route.request().postData() ?? "{}");
    const at = "2030-10-20T06:00:00Z";
    const log = (action: string, target: string, summary: string) => history.unshift({ id: history.length + 1, at, actorName: me.name, action, target, summary });
    if (url.pathname === `${api}/me`) return json(200, me);
    if (url.pathname === `${api}/history`) return json(200, history);
    if (url.pathname === `${api}/published` && method === "GET") return json(200, published.filter((p) => p.kpi === url.searchParams.get("kpi")));
    if (url.pathname === `${api}/published` && method === "POST") {
      if (!me.roles.includes("steward")) return json(403, { detail: "this needs the role steward" });
      const { kpi, days, horizon, note } = sent();
      const start = Date.parse(at);
      const points = Array.from({ length: horizon }, (_, i) => ({ t: start + (i + 1) * 86_400_000, v: 100 + i, lo: 90 + i, hi: 110 + i }));
      const made = { id: published.length + 1, kpi, days, horizon, note, step: 86_400_000, points, publisherName: me.name, publishedAt: at };
      published.unshift(made);
      log("insert", `published/${made.id}`, `published a forecast: ${note}`);
      return json(201, made);
    }
    if (url.pathname === `${api}/follows` && method === "GET") return json(200, follows);
    if (url.pathname === `${api}/follows` && method === "POST") {
      const made = { id: follows.length + 1, kpi: sent().kpi };
      follows.unshift(made);
      log("insert", `follows/${made.id}`, "followed an indicator");
      return json(201, made);
    }
    if (url.pathname.startsWith(`${api}/`) && url.pathname !== `${api}/forecasts`) {
      served.missing.push(`${method} ${url.pathname}`);
      return json(404, { detail: "no such route" });
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const body = route.request().postData();
      const answer = await transport({ method: route.request().method() as "GET", path: url.pathname + url.search, body: body ? JSON.parse(body) : undefined });
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
    // The CSP the static host sends a `ui` App (AP-142): WebAssembly may compile, nothing evals.
    const headers = file === "index.html" ? { "Content-Security-Policy": "script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:" } : undefined;
    return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? "application/octet-stream", body, headers });
  });
  return served;
}
