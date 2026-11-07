import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6095 (owner 2026-10-29): a list page is one compact line (title, description on hover, search,
// primary action) with the rows right below; a row opens by its name and keeps one ⋯ menu, with
// the forge file in it instead of a Source column; "Connect an AI assistant" sits behind a header
// button on the Endpoints page.

const API = "joinedcontext.com/v1alpha1";
const SOURCE = "https://git.example.sk/bbsk/config/src/branch/main";

const endpoint = (name: string, en: string, space: string) => ({
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name, namespace: "bbsk", title: { en } },
  spec: { contextSpaceRef: space, slug: name, audience: "public", enabledRepresentations: ["ngsi-ld", "ogc-features"] },
  status: { phase: "Live", sourceUrl: `${SOURCE}/endpoints/${name}.yaml` },
});

const ENDPOINTS = [
  endpoint("parking", "Parking", "otvorene-data"),
  endpoint("air-quality", "Air quality", "ovzdusie"),
  endpoint("events", "Cultural events", "kultura"),
];

const SPACES = [
  ["otvorene-data", "Open data", "bbsk-catalogue"],
  ["ovzdusie", "Air quality", "air-quality"],
  ["kultura", "Culture", "events"],
].map(([name, title, model]) => ({
  apiVersion: API,
  kind: "ContextSpace",
  metadata: { name, namespace: "bbsk", title: { en: title } },
  spec: { dataModelRef: model },
  status: { phase: "Live", sourceUrl: `${SOURCE}/spaces/${name}/space.yaml` },
}));

async function stub(page: Page, filled: boolean, list: "ok" | "loading" | "error" = "ok") {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s", username: "jana", name: "Jana", email: "jana@bbsk.sk", roles: ["portal-editor"] });
    }
    if (path.endsWith("/permissions/me")) return json({ project: "bbsk", bootstrap: true, grants: [] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path === "/api/v1/projects/bbsk/endpoints" && list === "loading") return; // never answers
    if (path === "/api/v1/projects/bbsk/endpoints" && list === "error") {
      return route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "The configuration store did not answer in time." }),
      });
    }
    if (path === "/api/v1/projects/bbsk/endpoints") return json({ apiVersion: API, kind: "List", items: filled ? ENDPOINTS : [] });
    if (path === "/api/v1/projects/bbsk/spaces") return json({ apiVersion: API, kind: "List", items: filled ? SPACES : [] });
    const usage = /\/spaces\/([^/]+)\/usage$/.exec(path);
    if (usage) return json({ entities: { "otvorene-data": 48210, ovzdusie: 1312, kultura: 0 }[usage[1]] ?? 0, broker: "main", observedAt: "2026-10-29T10:00:00Z" });
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

const dir = join("test-results", "T-6095");

for (const width of [1440, 400]) {
  test(`the Endpoints list is one header line, a grid and a row menu at ${width} px`, async ({ page }) => {
    mkdirSync(dir, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    await stub(page, true);
    await page.goto("/projects/bbsk/endpoints?lang=en");

    await expect(page.getByRole("heading", { level: 1, name: "Endpoints" })).toBeVisible();
    await expect(page.getByRole("searchbox", { name: "Find an endpoint" })).toBeVisible();
    await expect(page.getByRole("button", { name: "View data" })).toHaveCount(0);
    await expect(page.getByRole("region", { name: "Connect an AI assistant" })).toHaveCount(0);
    await page.screenshot({ path: join(dir, `endpoints-filled-${width}.png`) });
    expect(await axeViolations(page)).toEqual([]);

    // The description shows on hover of the info mark.
    await page.getByRole("button", { name: "About this page" }).hover();
    await expect(page.getByRole("tooltip")).toBeVisible();
    // The open description covers none of the header's controls, at 400 px neither (T-6095 review).
    const tip = (await page.getByRole("tooltip").boundingBox())!;
    for (const control of [
      page.getByRole("searchbox", { name: "Find an endpoint" }),
      page.getByRole("button", { name: "Connect AI" }),
      page.getByRole("button", { name: "New endpoint" }),
    ]) {
      const box = (await control.boundingBox())!;
      const overlaps = tip.x < box.x + box.width && box.x < tip.x + tip.width && tip.y < box.y + box.height && box.y < tip.y + tip.height;
      expect(overlaps).toBe(false);
    }
    await page.screenshot({ path: join(dir, `endpoints-description-${width}.png`) });
    await page.mouse.move(0, 0);

    // The search narrows the rows; a search matching nothing says so and offers the way back.
    const search = page.getByRole("searchbox", { name: "Find an endpoint" });
    await search.fill("air");
    await expect(page.getByRole("link", { name: "Air quality" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Parking" })).toHaveCount(0);
    await search.fill("tram");
    await expect(page.getByText("No endpoint name or title holds “tram”.")).toBeVisible();
    await page.screenshot({ path: join(dir, `endpoints-nomatch-${width}.png`) });
    await page.getByRole("button", { name: "Clear the search" }).click();
    await expect(search).toHaveValue("");
    await expect(page.getByRole("link", { name: "Parking" })).toBeVisible();

    // One menu: Open, Edit, View source (Git) among the rest.
    await page.getByRole("button", { name: /More actions.*Parking/ }).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitem", { name: "Open" })).toBeVisible();
    await expect(menu.getByRole("menuitem", { name: "View source (Git)" })).toBeVisible();
    await page.screenshot({ path: join(dir, `endpoints-menu-${width}.png`) });
    await menu.getByRole("menuitem", { name: "Open" }).click();
    await expect(page).toHaveURL(/\/projects\/bbsk\/endpoints\/parking/);

    // Connect AI opens the same content as a dialog.
    await page.goto("/projects/bbsk/endpoints?lang=en");
    await page.getByRole("button", { name: "Connect AI" }).click();
    const dialog = page.getByRole("dialog", { name: "Connect an AI assistant" });
    await expect(dialog.getByLabel("Claude Code command")).toBeVisible();
    await page.screenshot({ path: join(dir, `endpoints-connect-${width}.png`) });
    expect(await axeViolations(page)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });

  test(`an empty Endpoints list says what to do next at ${width} px`, async ({ page }) => {
    mkdirSync(dir, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false);
    await page.goto("/projects/bbsk/endpoints?lang=en");
    await expect(page.getByText("This project publishes no endpoint yet.")).toBeVisible();
    await expect(page.getByRole("searchbox", { name: "Find an endpoint" })).toHaveCount(0);
    // Own and shared Endpoints are one grid (T-6169): the empty card offers the way to a shared one.
    await expect(page.getByRole("link", { name: "See what other projects publish" })).toBeVisible();
    await page.screenshot({ path: join(dir, `endpoints-empty-${width}.png`) });
  });

  test(`the Endpoints list shows loading and a failed load under the compact header at ${width} px`, async ({ page }) => {
    mkdirSync(dir, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    await stub(page, true, "loading");
    await page.goto("/projects/bbsk/endpoints?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Endpoints" })).toBeVisible();
    await expect(page.getByRole("table", { name: "Endpoints" })).toHaveAttribute("aria-busy", "true");
    await page.screenshot({ path: join(dir, `endpoints-loading-${width}.png`) });

    await page.unrouteAll({ behavior: "ignoreErrors" });
    await stub(page, true, "error");
    await page.goto("/projects/bbsk/endpoints?lang=en");
    await expect(page.getByText("The configuration store did not answer in time.")).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
    await page.screenshot({ path: join(dir, `endpoints-error-${width}.png`) });
    expect(await axeViolations(page)).toEqual([]);
  });

  test(`the Context Spaces list has no Look inside and no Source column at ${width} px`, async ({ page }) => {
    mkdirSync(dir, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    await stub(page, true);
    await page.goto("/projects/bbsk/spaces?lang=en");
    await expect(page.getByRole("link", { name: "Air quality" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Look inside" })).toHaveCount(0);
    await expect(page.getByRole("columnheader", { name: "Source" })).toHaveCount(0);
    await page.screenshot({ path: join(dir, `spaces-filled-${width}.png`) });
    expect(await axeViolations(page)).toEqual([]);
    await page.getByRole("button", { name: /More actions.*Air quality/ }).click();
    await expect(page.getByRole("menuitem", { name: "Open" })).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "View source (Git)" })).toBeVisible();
    await page.screenshot({ path: join(dir, `spaces-menu-${width}.png`) });
  });
}
