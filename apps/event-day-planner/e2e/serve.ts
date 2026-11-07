import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { EVENTS } from "../src/fixtures/events";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "eventdayplanner";
// `portal`: where the entity panel links an event (SDK-40); shown, never followed.
const CONFIG = { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "event-day-planner", language: "en", portal: "https://portal.test/projects/helsinki" };
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
export const VIEWER = { id: "u-1", name: "Vierailija", roles: ["viewer"] };
export const STEWARD = { id: "u-2", name: "Ylläpitäjä", roles: ["viewer", "steward"] };

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, me: { id: string; name: string; roles: string[] } = VIEWER): Promise<Served> {
  const transport = stubTransport({
    entities: EVENTS,
    access: { permissions: [{ resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  // One day this viewer shared before, the featured events a steward keeps, and the history.
  const days = [{ id: 1, code: "abc123def456", day: "2030-10-20", events: 2, createdAt: "2030-10-19T09:00:00Z" }];
  const featured: { id: number; eventId: string; note: string }[] = [];
  const history: unknown[] = [];
  page.on("pageerror", (error) => served.problems.push(error.message));
  // Sunday 20 October 2030, 08:00 in Helsinki: the fixtures' day, before its first event.
  await page.clock.setFixedTime(new Date("2030-10-20T05:00:00Z"));

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
    // The App's own server (T-3347, T-3826), deciding by `me` as the real one does; any other call is missing.
    const api = `/apps/${CONFIG.appName}/api`;
    const method = route.request().method();
    const json = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const steward = me.roles.includes("steward");
    const record = (action: string, target: string, summary: string) =>
      history.unshift({ id: history.length + 1, at: "2030-10-20T05:00:00Z", actorName: me.name, action, target, summary });
    if (url.pathname === `${api}/me` && method === "GET") return json(200, me);
    if (url.pathname === `${api}/days` && method === "GET") return json(200, days);
    if (url.pathname === `${api}/featured` && method === "GET") return json(200, featured);
    if (url.pathname === `${api}/history` && method === "GET") return json(200, history);
    if (url.pathname === `${api}/featured` && method === "POST") {
      if (!steward) return json(403, { detail: "this needs the role steward" });
      const made = { id: featured.length + 1, ...(JSON.parse(route.request().postData() ?? "{}") as { eventId: string; note: string }) };
      featured.push(made);
      record("insert", `featured/${made.id}`, `featured ${made.eventId}`);
      return json(201, made);
    }
    const one = url.pathname.match(new RegExp(`^${api}/featured/(\\d+)$`));
    if (one && method === "PUT") {
      if (!steward) return json(403, { detail: "this needs the role steward" });
      const found = featured.find((f) => f.id === Number(one[1]));
      if (!found) return json(404, { detail: "no such featured event" });
      found.note = (JSON.parse(route.request().postData() ?? "{}") as { note: string }).note;
      record("update", `featured/${found.id}`, "changed the note");
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
    return route.fulfill({
      status: 200,
      contentType: TYPES[extname(file)] ?? "application/octet-stream",
      headers: { "content-security-policy": CSP },
      body,
    });
  });
  return served;
}
