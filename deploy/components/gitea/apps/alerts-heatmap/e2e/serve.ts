import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { ALERTS } from "../src/fixtures/alerts";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "alertsheatmap";
// `portal`: where the entity panel links an alert for editing (SDK-40); shown, never followed.
const CONFIG = { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "alerts-heatmap", portal: "https://portal.test/projects/helsinki" };
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

/** The App's server (T-3351) as the page sees it: the weeks it keeps and the reports saved, in memory. */
const WEEKS = {
  weeks: [
    { week: "2030-10-21", alerts: 3, places: [], computed_at: "2030-10-21T10:00:00Z" },
    { week: "2030-10-14", alerts: 5, places: [{ name: "Mannerheimintie, Helsinki", lon: 24.9384, lat: 60.1699, count: 3 }], computed_at: "2030-10-21T10:00:00Z" },
  ],
  stale: false,
};

/** Who reads (T-3828): a viewer keeps filters, a steward also acknowledges alerts. */
export const VIEWER = { id: "u-viewer", name: "Vera Viewer", roles: ["viewer"] };
export const STEWARD = { id: "u-steward", name: "Sara Steward", roles: ["viewer", "steward"] };
type Me = typeof VIEWER;

function appServer(me: Me) {
  const reports: Array<Record<string, unknown>> = [];
  const pictures = new Set<string>();
  const filters: Array<Record<string, unknown>> = [];
  const acks: Array<Record<string, unknown>> = [];
  const history: Array<Record<string, unknown>> = [];
  let next = 1;
  const log = (action: string, summary: string) =>
    history.unshift({ id: next++, at: "2030-10-21T09:00:00Z", actorName: me.name, action, target: "", summary });
  return (method: string, path: string, body: unknown): { status: number; body: unknown } => {
    if (method === "GET" && path === "/me") return { status: 200, body: me };
    if (method === "GET" && path === "/history") return { status: 200, body: history };
    if (method === "GET" && path === "/filters") return { status: 200, body: filters };
    if (method === "POST" && path === "/filters") {
      const filter = { ...(body as object), id: next++ };
      filters.unshift(filter);
      log("insert", `kept the filter ${filter.name as string}`);
      return { status: 201, body: filter };
    }
    if (method === "GET" && path === "/acks") return { status: 200, body: acks };
    if (method === "POST" && path === "/acks") {
      if (!me.roles.includes("steward")) return { status: 403, body: { title: "Forbidden", detail: "a steward acknowledges alerts" } };
      const ack = { ...(body as object), id: next++, ownerName: me.name };
      acks.unshift({ ...ack, at: "2030-10-21T09:00:00Z" });
      log("insert", `acknowledged ${ack.comment as string}`);
      return { status: 201, body: ack };
    }
    const gone = /^\/(filters|acks)\/(\d+)$/.exec(path);
    if (method === "DELETE" && gone) {
      const list = gone[1] === "filters" ? filters : acks;
      list.splice(list.findIndex((one) => one.id === Number(gone[2])), 1);
      log("delete", `deleted ${gone[1]} ${gone[2]}`);
      return { status: 204, body: null };
    }
    if (method === "GET" && path === "/weeks") return { status: 200, body: WEEKS };
    if (method === "GET" && path === "/reports") return { status: 200, body: [...reports].reverse() };
    if (method === "POST" && path === "/reports") {
      const report = { ...(body as object), id: reports.length + 1, has_snapshot: false, created_at: "2030-10-21T09:00:00Z" };
      reports.push(report);
      return { status: 201, body: report };
    }
    const snapshot = /^\/reports\/(\d+)\/snapshot$/.exec(path);
    const report = snapshot ? reports.find((r) => r.id === Number(snapshot[1])) : undefined;
    if (!snapshot || !report) return { status: 404, body: { title: "Not Found", detail: "no such report" } };
    if (method === "POST") {
      report.has_snapshot = true;
      pictures.add(String(report.id));
      return { status: 200, body: { url: `http://portal.test/store/reports/${report.id}/map.png`, method: "PUT" } };
    }
    return { status: 200, body: { url: `http://portal.test/store/reports/${report.id}/map.png` } };
  };
}

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, entities = ALERTS, me: Me = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities,
    access: { permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  const api = appServer(me);
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
    if (url.pathname.startsWith("/apps/alerts-heatmap/api/")) {
      const body = route.request().postData();
      const answer = api(route.request().method(), url.pathname.slice("/apps/alerts-heatmap/api".length), body ? JSON.parse(body) : undefined);
      if (answer.status === 204) return route.fulfill({ status: 204, body: "" });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body) });
    }
    // The store's presigned URLs: a picture goes there from the browser, never through the server.
    if (url.pathname.startsWith("/store/")) {
      return route.fulfill({ status: 200, contentType: route.request().method() === "PUT" ? "text/plain" : "image/png", body: "" });
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
