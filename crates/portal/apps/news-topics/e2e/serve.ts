import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { ARTICLES as NEWS } from "../src/fixtures/news";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "newstopics";
// `portal`: where the entity panel links an article (SDK-40); shown, never followed.
const CONFIG = { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "news-topics", portal: "https://portal.test/projects/helsinki" };
const TYPES: Record<string, string> = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
};

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** The App's server (T-3352) as the page sees it: two kept weeks and their corpus files. */
const KEPT = {
  weeks: [
    { week: "2026-W42", articles: 3, computed_at: "2026-10-18T05:00:00Z", topics: [{ topic: 0, share: 1, articles: 3, keywords: [{ term: "raitiotie", weight: 0.9 }, { term: "liikenne", weight: 0.6 }] }] },
    { week: "2026-W41", articles: 2, computed_at: "2026-10-18T05:00:00Z", topics: [{ topic: 0, share: 0.5, articles: 1, keywords: [{ term: "kirjasto", weight: 0.8 }] }, { topic: 1, share: 0.5, articles: 1, keywords: [{ term: "uimahalli", weight: 0.7 }] }] },
  ],
  stale: false,
};

/** Who reads (T-3828): a viewer follows topics, a steward also names and merges them. */
export const VIEWER = { id: "u-viewer", name: "Vera Viewer", roles: ["viewer"] };
export const STEWARD = { id: "u-steward", name: "Sara Steward", roles: ["viewer", "steward"] };
type Me = typeof VIEWER;

/** The App's role routes in memory, deciding as the server does. */
function roleServer(me: Me) {
  const follows: Array<{ id: number; term: string }> = [];
  const names: Array<{ id: number; term: string; name: string; ownerName: string }> = [];
  const history: Array<Record<string, unknown>> = [];
  let next = 1;
  const log = (action: string, summary: string) =>
    history.unshift({ id: next++, at: "2026-10-18T06:00:00Z", actorName: me.name, action, target: "", summary });
  return (method: string, path: string, body: Record<string, string> | undefined): { status: number; body: unknown } | null => {
    if (method === "GET" && path === "/me") return { status: 200, body: me };
    if (method === "GET" && path === "/history") return { status: 200, body: history };
    if (method === "GET" && path === "/follows") return { status: 200, body: follows };
    if (method === "POST" && path === "/follows") {
      const follow = { id: next++, term: body!.term };
      follows.unshift(follow);
      log("insert", `followed ${follow.term}`);
      return { status: 201, body: follow };
    }
    if (method === "GET" && path === "/names") return { status: 200, body: names };
    if (method === "POST" && path === "/names") {
      if (!me.roles.includes("steward")) return { status: 403, body: { title: "Forbidden", detail: "a steward names topics" } };
      const entry = { id: next++, term: body!.term, name: body!.name, ownerName: me.name };
      names.splice(0, names.length, ...names.filter((other) => other.term !== entry.term), entry);
      log("insert", `named the topic of ${entry.term} ${entry.name}`);
      return { status: 201, body: entry };
    }
    const gone = /^\/(follows|names)\/(\d+)$/.exec(path);
    if (method === "DELETE" && gone) {
      const list: Array<{ id: number }> = gone[1] === "follows" ? follows : names;
      list.splice(list.findIndex((one) => one.id === Number(gone[2])), 1);
      log("delete", `deleted ${gone[1]} ${gone[2]}`);
      return { status: 204, body: null };
    }
    return null;
  };
}

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page, me: Me = VIEWER): Promise<Served> {
  const roles = roleServer(me);
  const transport = stubTransport({
    entities: NEWS,
    access: { permissions: [{ resource: { type: "Article" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));
  // The page opens on today in Helsinki and filters by period (default 8 weeks): fixed to the
  // date the fixtures fall on, so the weekly chart and topics have articles to analyse.
  await page.clock.setFixedTime(new Date("2026-10-18T06:00:00Z"));

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://portal.test") {
      served.outside.push(url.href);
      return route.abort();
    }
    // A published app is served at the root of its own host and calls its endpoint unprefixed,
    // where the edge sets the session as the bearer (T-2670, T-2838); the stub answers that path.
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const body = route.request().postData();
      const path = url.pathname + url.search;
      const answer = await transport({ method: route.request().method() as "GET", path, body: body ? JSON.parse(body) : undefined });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body ?? null) });
    }
    if (url.pathname === "/apps/news-topics/api/weeks") {
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(KEPT) });
    }
    if (url.pathname.startsWith("/apps/news-topics/api/")) {
      const body = route.request().postData();
      const answer = roles(route.request().method(), url.pathname.slice("/apps/news-topics/api".length), body ? JSON.parse(body) : undefined);
      if (answer?.status === 204) return route.fulfill({ status: 204, body: "" });
      if (answer) return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body) });
    }
    const corpus = /^\/apps\/news-topics\/api\/weeks\/(\d{4}-W\d{2})\/corpus$/.exec(url.pathname);
    if (corpus) {
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ url: `http://portal.test/store/corpus/${corpus[1]}.json` }) });
    }
    // The store's presigned URL: the week's articles come from there, never through the server.
    if (url.pathname.startsWith("/store/corpus/")) {
      return route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
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
    return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? "application/octet-stream", body });
  });
  return served;
}
