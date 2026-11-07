import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import type { FnHandler } from "@joinedcontext/sdk/server";
import { fakeContext, stubTransport } from "@joinedcontext/sdk/testing";
import { ROWS } from "../src/fixtures";

// The built bundle; the build lane points `dist/` at the bundle it just built (T-2827).
const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://app.test/";
const SLUG = "app";
// `portal`: where the entity panel links an alert for editing (SDK-40); shown, never followed.
const CONFIG = { slug: SLUG, orgDomain: "example.org", space: "demo", transport: "origin", appName: "app", portal: "https://portal.example.org/projects/demo" };
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };
const FUNCTION = /^\/api\/functions\/([a-z][a-z0-9-]{0,39})$/;

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** The application's own function, run on the fixtures the way the functions runtime runs it. */
async function callFunction(name: string, body: unknown): Promise<{ status: number; body: unknown }> {
  const file = fileURLToPath(new URL(`../functions/${name}.ts`, import.meta.url));
  if (!existsSync(file)) return { status: 404, body: { title: "Not Found", status: 404, detail: `Function ${name} not found` } };
  const handler = (await import(file)).default as FnHandler;
  try {
    const answer = await handler({ method: "POST", query: {}, body, user: null }, fakeContext({ entities: ROWS }));
    return { status: answer.status ?? 200, body: answer.body ?? null };
  } catch (error) {
    return { status: 500, body: { title: "Function failed", status: 500, detail: String(error) } };
  }
}

/**
 * Serves the bundle at the root of the App's own host, with `#jc-config` filled as the static
 * host fills it and the SDK's stub answering its endpoint, read-only, from `src/fixtures.ts`.
 * Nothing leaves the page: any other host is refused and reported.
 */
export async function serve(page: Page): Promise<Served> {
  const transport = stubTransport({
    entities: ROWS,
    access: { permissions: [{ resource: { type: "*" }, actions: ["queryEntity", "retrieveEntity", "queryTemporal", "retrieveTemporal"], attributes: "*" }], prohibitions: [] },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));

  await page.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== new URL(BASE).origin) {
      served.outside.push(url.href);
      return route.abort();
    }
    const body = request.postData();
    const fn = FUNCTION.exec(url.pathname);
    if (fn && request.method() === "POST") {
      const answer = await callFunction(fn[1], body ? JSON.parse(body) : undefined);
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body) });
    }
    if (url.pathname.startsWith(`/api/endpoint/${SLUG}/`)) {
      const answer = await transport({ method: request.method() as "GET", path: url.pathname + url.search, body: body ? JSON.parse(body) : undefined });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body ?? null) });
    }
    const file = normalize(url.pathname.slice(1) || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) {
      served.missing.push(url.pathname);
      return route.fulfill({ status: 404, body: "" });
    }
    let content = readFileSync(join(DIST, file));
    if (file === "index.html") {
      content = Buffer.from(
        content
          .toString("utf8")
          .replace('<script id="jc-config" type="application/json"></script>', `<script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`),
      );
    }
    return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? "application/octet-stream", body: content });
  });
  return served;
}
