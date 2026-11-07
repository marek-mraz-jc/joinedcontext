import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// A public Endpoint's API documentation (T-3265, EP-99), opened by somebody who is not signed in:
// the page reads the gateway's generated OpenAPI anonymously and asks the Portal for nothing.

const DOC = {
  openapi: "3.1.0",
  info: { title: "Helsinki city bikes", description: "Stations and free bikes.", version: "1.0.0" },
  paths: {
    "/ngsi-ld/v1/entities": {
      get: {
        summary: "The entities of a type",
        parameters: [{ name: "type", in: "query", description: "The entity type", schema: { type: "string", enum: ["BikeHireDockingStation"] } }],
      },
    },
  },
  components: { schemas: { BikeHireDockingStation: { type: "object", properties: { availableBikeNumber: { type: "integer" } } } } },
};

test.describe("an endpoint's public documentation", () => {
  test("a visitor reads the operations, an example call and the types, and a slug that is gone says so", async ({ page }) => {
    const asked: string[] = [];
    await page.route("**/api/v1/**", async (route) => {
      const url = new URL(route.request().url());
      asked.push(url.pathname);
      const unauthorized = url.pathname.endsWith("/auth/me");
      await route.fulfill({
        status: unauthorized ? 401 : 404,
        contentType: "application/problem+json",
        body: JSON.stringify({ title: unauthorized ? "Unauthorized" : "Not Found", status: unauthorized ? 401 : 404 }),
      });
    });
    await page.route("**/api/endpoint/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.startsWith("/api/endpoint/gone/")) {
        return route.fulfill({ status: 404, contentType: "application/problem+json", body: JSON.stringify({ title: "Not Found", status: 404 }) });
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(DOC) });
    });

    await page.goto("/d/k7m2qz4tv6xh3n5jb2ryd3wcfa?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Helsinki city bikes" })).toBeVisible();
    await expect(page.getByRole("region", { name: "GET /ngsi-ld/v1/entities" }).getByText(/curl .*type=BikeHireDockingStation/)).toBeVisible();
    await expect(page.getByRole("region", { name: "BikeHireDockingStation" })).toContainText("availableBikeNumber");
    expect(await axeViolations(page)).toEqual([]);
    expect(asked.filter((path) => path.startsWith("/api/v1/projects"))).toEqual([]);

    await page.goto("/d/gone?lang=en");
    await expect(page.getByText("This endpoint's documentation cannot be read: it does not exist, or you may not read it.")).toBeVisible();
  });
});

// T-5111: the developer page of a public Endpoint: licence, rate limit, a sample per surface,
// try-it sent without the visitor's cookie, at 1440 and 400 px.
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const BASE = `https://api.dev.example/api/endpoint/${SLUG}/`;
const STANDARDS = {
  slug: SLUG,
  types: ["BikeHireDockingStation"],
  entityType: "BikeHireDockingStation",
  base: BASE,
  served: ["ngsi-ld", "csv", "websocket", "ogc-features", "odata", "mcp"],
  licence: { code: "CC_BY_4_0", title: "Creative Commons Attribution 4.0", url: "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0" },
  requestsPerMinute: 600,
  standards: [
    { representation: "ngsi-ld", fields: ["availableBikeNumber"], examples: [], first: `${BASE}ngsi-ld/v1/entities?type=BikeHireDockingStation&limit=5` },
    { representation: "ogc-features", fields: [], examples: [], first: `${BASE}ogc/features/collections/BikeHireDockingStation/items?limit=5` },
  ],
};

test.describe("an endpoint's developer page", () => {
  test("a developer reads the terms, switches samples and tries a call, refused past the limit", async ({ page }, info) => {
    const cookies: (string | undefined)[] = [];
    let limited = false;
    await page.route("**/api/v1/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/standards")) return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(STANDARDS) });
      const unauthorized = url.pathname.endsWith("/auth/me");
      return route.fulfill({ status: unauthorized ? 401 : 404, contentType: "application/problem+json", body: JSON.stringify({ title: "Not Found", status: 404 }) });
    });
    await page.route("**/api/endpoint/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/openapi.json")) return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(DOC) });
      cookies.push(await route.request().headerValue("cookie") ?? undefined);
      if (limited) return route.fulfill({ status: 429, contentType: "application/problem+json", body: JSON.stringify({ title: "Too Many Requests", status: 429 }) });
      return route.fulfill({ status: 200, contentType: "application/ld+json", body: JSON.stringify([{ id: "urn:ngsi-ld:BikeHireDockingStation:001", type: "BikeHireDockingStation", availableBikeNumber: { type: "Property", value: 7 } }]) });
    });
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`/d/${SLUG}?lang=en`);
      const panel = page.getByTestId("developer-panel");
      await expect(panel.getByTestId("developer-limit")).toContainText("600 requests a minute per address");
      await expect(panel.getByRole("link", { name: "Creative Commons Attribution 4.0" })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`developer-${width}-terms.png`), fullPage: true });
      await panel.getByRole("button", { name: "Try it" }).click();
      await expect(panel.getByTestId("try-it-answer")).toContainText("BikeHireDockingStation:001");
      await panel.getByRole("tab", { name: "OGC API Features" }).click();
      await expect(panel.getByRole("heading", { name: "QGIS" })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`developer-${width}-ogc.png`), fullPage: true });
      const operation = page.getByRole("region", { name: "GET /ngsi-ld/v1/entities" });
      await operation.getByRole("textbox", { name: "Request" }).fill("/../../api/v1/me");
      await expect(operation.getByRole("button", { name: "Try it" })).toHaveAttribute("aria-disabled", "true");
      await operation.screenshot({ path: info.outputPath(`developer-${width}-refused-path.png`) });
      limited = true;
      await operation.getByRole("textbox", { name: "Request" }).fill("/ngsi-ld/v1/types");
      await operation.getByRole("button", { name: "Try it" }).click();
      await expect(operation.getByText(/too many requests\. Wait a minute/)).toBeVisible();
      await operation.screenshot({ path: info.outputPath(`developer-${width}-limited.png`) });
      limited = false;
      expect(await axeViolations(page)).toEqual([]);
      const scrolls = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
      expect(scrolls, `no sideways scroll at ${width}`).toBe(false);
    }
    expect(cookies.every((cookie) => cookie === undefined), "try-it sends no cookie").toBe(true);
  });
});

// T-5630: the page asks the gateway in its own language, so a Slovak visitor reads a Slovak heading.
test.describe("an endpoint's developer page in the visitor's language", () => {
  test("the sk page heads in Slovak and the en page in English", async ({ page }, info) => {
    await page.route("**/api/v1/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/standards")) return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(STANDARDS) });
      const unauthorized = url.pathname.endsWith("/auth/me");
      return route.fulfill({ status: unauthorized ? 401 : 404, contentType: "application/problem+json", body: JSON.stringify({ title: "Not Found", status: 404 }) });
    });
    await page.route("**/api/endpoint/**", async (route) => {
      const slovak = (await route.request().headerValue("accept-language")) === "sk";
      const title = slovak ? { title: "Mestské bicykle v Helsinkách", description: "Stanice a voľné bicykle." } : {};
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ ...DOC, info: { ...DOC.info, ...title } }) });
    });
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      for (const [language, title, lead] of [
        ["sk", "Mestské bicykle v Helsinkách", "Stanice a voľné bicykle."],
        ["en", "Helsinki city bikes", "Stations and free bikes."],
      ]) {
        await page.goto(`/d/${SLUG}?lang=${language}`);
        await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
        await expect(page.getByText(lead)).toBeVisible();
        await page.screenshot({ path: info.outputPath(`developer-${language}-${width}.png`) });
      }
    }
  });
});
