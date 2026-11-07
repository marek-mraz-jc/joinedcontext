import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5014: a `<pre>` that scrolls is reachable by keyboard and named (CodeBlock), at desktop and
// phone width. jsdom has no layout and axe skips scrollable-region-focusable there; a browser
// measures it.

const API = "joinedcontext.com/v1alpha1";
const dir = join("test-results", "T-5014");

const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "helsinki", labels: { "joinedcontext.com/space": "liikenne" } },
  spec: {
    contextSpaceRef: "liikenne",
    slug: "abcdefghijklmnopqrstuvwxyz",
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "mcp"],
  },
  status: { phase: "Live" },
};

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/** Every block that scrolls takes focus, carries a name, and axe finds nothing on the page. */
async function blocksAreReachable(page: Page) {
  const scrolling = await page.locator("pre").evaluateAll((blocks) =>
    blocks
      .filter((pre) => pre.scrollWidth > pre.clientWidth || pre.scrollHeight > pre.clientHeight)
      .map((pre) => ({ tabIndex: pre.tabIndex, name: pre.getAttribute("aria-label") ?? pre.getAttribute("aria-labelledby") })),
  );
  for (const block of scrolling) {
    expect(block.tabIndex).toBe(0);
    expect(block.name).toBeTruthy();
  }
  for (const block of await page.locator("pre").all()) {
    await block.focus();
    await expect(block).toBeFocused();
  }
  expect(await axeViolations(page)).toEqual([]);
}

for (const width of [400, 1440]) {
  test.describe(`code blocks at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 1024 });
      mkdirSync(dir, { recursive: true });
    });

    test("an Endpoint page's code blocks are reachable by keyboard", async ({ page }) => {
      await page.route("**/api/v1/**", (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path.endsWith("/auth/me")) {
          return json(route, { subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
        }
        if (path === "/api/v1/projects") return json(route, { apiVersion: API, kind: "List", items: [{ name: "helsinki" }] });
        if (path === "/api/v1/projects/helsinki/endpoints/bikes") return json(route, ENDPOINT);
        return json(route, { apiVersion: API, kind: "List", items: [] });
      });
      await page.route("**/api/endpoint/**", (route) => json(route, {}, 404));

      await page.goto("/projects/helsinki/endpoints/bikes?lang=en");
      await expect(page.getByRole("region", { name: "Connect an AI assistant" })).toBeVisible();
      await expect(page.getByRole("group", { name: "Claude Code command" })).toBeVisible();
      await blocksAreReachable(page);
      await page.screenshot({ path: join(dir, `endpoint-${width}.png`), fullPage: true });
    });

    test("the public connect page's command is reachable by keyboard", async ({ page }) => {
      await page.route("**/api/v1/**", (route) => {
        const unauthorized = new URL(route.request().url()).pathname.endsWith("/auth/me");
        return json(route, { title: unauthorized ? "Unauthorized" : "Not Found" }, unauthorized ? 401 : 404);
      });
      await page.route("**/api/mcp/bbsk/otvorene-data", (route) => {
        const { method } = route.request().postDataJSON() as { method: string };
        const result =
          method === "initialize"
            ? { serverInfo: { name: "otvorene-data", title: "Banská Bystrica Region, open data" }, instructions: "Read-only open data." }
            : { tools: [{ name: "query_entities", description: "Entities of one type.", annotations: { readOnlyHint: true } }] };
        return json(route, { jsonrpc: "2.0", id: 1, result });
      });

      await page.goto("/connect/bbsk/otvorene-data?lang=en");
      await expect(page.getByText("query_entities", { exact: true })).toBeVisible();
      await expect(page.getByRole("group", { name: "Code" }).first()).toBeVisible();
      await blocksAreReachable(page);
      await page.screenshot({ path: join(dir, `connect-${width}.png`), fullPage: true });
    });
  });
}
