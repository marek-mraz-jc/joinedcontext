import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import { STATIONS } from "../src/fixtures/stations";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
export const BASE = "http://portal.test/";
const SLUG = "helsinkibikes";
// `portal`: where the entity panel links a station for editing (SDK-40); shown, never followed.
const CONFIG = { slug: SLUG, orgDomain: "hel.fi", space: "helsinki", transport: "origin", appName: "helsinki-bikes", portal: "https://portal.test/projects/helsinki" };
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };

/** What the served page did that it should not: a host it called, a file it missed, an error it threw. */
export interface Served {
  outside: string[];
  missing: string[];
  problems: string[];
}

/** Serves the built bundle at the root of the App's own host, with the SDK's stub answering its endpoint. */
export async function serve(page: Page): Promise<Served> {
  const transport = stubTransport({
    entities: STATIONS,
    access: { permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] },
  });
  const served: Served = { outside: [], missing: [], problems: [] };
  page.on("pageerror", (error) => served.problems.push(error.message));

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
