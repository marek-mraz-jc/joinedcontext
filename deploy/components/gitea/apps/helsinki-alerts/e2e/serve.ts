import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, type Page } from "@playwright/test";
import { stubTransport } from "@joinedcontext/sdk/testing";
import type { AccessDocument } from "@joinedcontext/sdk";
import { ALERTS } from "../src/fixtures/alerts";
import { SCHEMA } from "../src/fixtures/schema";

const DIST = fileURLToPath(new URL("../dist/", import.meta.url));
/** The App is served at the root of its own host (T-2838). */
const APP = "/";
export const SLUG = "helsinkialerts";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };

interface Call {
  method: string;
  path: string;
  body: unknown;
}

/** Serves the built bundle as the static host does, opens the Alerts page,, `#jc-config` filled for `role`, and the endpoint from the SDK's stub. */
export async function serve(page: Page, role: "viewer" | "steward", access: AccessDocument) {
  const transport = stubTransport({ entities: ALERTS, schema: SCHEMA, access });
  const config = {
    slug: SLUG,
    orgDomain: "hel.fi",
    space: "helsinki",
    transport: "origin",
    appName: "helsinki-alerts",
    user: { id: `demo.${role}`, name: `Demo ${role}`, roles: [role] },
    // Where the entity panel links an alert (SDK-40); shown, never followed.
    portal: "https://portal.test/projects/helsinki",
  };
  const writes: Call[] = [];
  const outside: string[] = [];
  const problems: string[] = [];
  page.on("pageerror", (error) => problems.push(error.message));
  page.on("dialog", (dialog) => void dialog.accept());

  await page.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== "http://portal.test") {
      outside.push(url.href);
      return route.abort();
    }
    // A published app calls its endpoint unprefixed on its own host, where the edge sets the
    // session as the bearer (T-2670, T-2838); the stub answers the gateway's path.
    if (url.pathname.startsWith(`${APP}api/endpoint/${SLUG}/`)) {
      const raw = request.postData();
      const body = raw ? JSON.parse(raw) : undefined;
      const path = url.pathname.slice(APP.length - 1);
      if (request.method() !== "GET") writes.push({ method: request.method(), path, body });
      const answer = await transport({ method: request.method() as "GET", path: path + url.search, body });
      return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify(answer.body ?? null) });
    }
    const file = normalize(url.pathname.slice(APP.length) || "index.html");
    if (file.startsWith("..") || !existsSync(join(DIST, file))) return route.fulfill({ status: 404, body: "" });
    let body = readFileSync(join(DIST, file));
    if (file === "index.html") {
      body = Buffer.from(
        body.toString("utf8").replace('<script id="jc-config" type="application/json"></script>', `<script id="jc-config" type="application/json">${JSON.stringify(config)}</script>`),
      );
    }
    return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? "application/octet-stream", body });
  });

  await page.goto(`http://portal.test${APP}#alerts`);
  const alerts = page.getByRole("region", { name: "Alerts" });
  await expect(alerts.getByRole("table").getByText("Mannerheimintie resurfacing")).toBeVisible();
  return { alerts, writes, outside, problems };
}
