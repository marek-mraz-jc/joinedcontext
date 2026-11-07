import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { axeViolations } from "./axe";

// The plain-HTML example of the SDK (T-2597, AP-83; sdk/examples since T-2923) served the way the Portal static host serves it:
// the folder as it is, `#jc-config` written first thing in the head, the host's Content Security
// Policy on the page, and the endpoint stubbed with the five events the node tests use. No
// server: the page's own origin is answered from the folder.

const ORIGIN = "https://apps.test";
const APP = join(import.meta.dirname, "../../sdk/examples/plain-html-events");
const EVENTS: unknown[] = JSON.parse(readFileSync(join(APP, "test/events.json"), "utf8"));
// Byte for byte what src/apps/static_host.rs sends for an app that is not embeddable.
const CSP =
  "default-src 'self'; base-uri 'self'; object-src 'none'; script-src 'self'; " +
  "style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; " +
  "form-action 'self'; connect-src 'self'; frame-ancestors 'none'";
// The host lists every endpoint of the space and names the first by name as the primary; the
// events endpoint is not it (static_host.rs `served_config`).
const CONFIG = {
  slug: "alerts-slug",
  space: "helsinki",
  transport: "origin",
  appName: "helsinki-events",
  endpoints: [
    { name: "helsinki-alerts", slug: "alerts-slug", space: "helsinki", types: ["Event"] },
    { name: "helsinki-events", slug: "events-slug", space: "helsinki", types: ["Event"] },
  ],
};
const TYPES: Record<string, string> = {
  html: "text/html",
  js: "text/javascript",
  css: "text/css",
};

/** Serves the folder under its published path with the host's CSP; endpoint reads land in `reads`. */
async function serve(page: Page, reads: URL[] = []): Promise<void> {
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) {
      // AP-11: the page reaches no other host. Refused here and reported by the caller.
      return route.abort();
    }
    if (url.pathname.startsWith("/api/endpoint/")) {
      reads.push(url);
      return route.fulfill({ status: 200, contentType: "application/ld+json", json: EVENTS });
    }
    const file = url.pathname.replace(/^\/apps\/helsinki-events\//, "") || "index.html";
    if (!/^[a-z]+\.(html|js|css)$/.test(file)) {
      return route.fulfill({ status: 404, body: "not found" });
    }
    let body = readFileSync(join(APP, file), "utf8");
    if (file === "index.html") {
      body = body.replace(
        "<head>",
        `<head><script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`,
      );
    }
    return route.fulfill({
      status: 200,
      headers: { "content-type": TYPES[file.split(".").pop() ?? ""], "content-security-policy": CSP },
      body,
    });
  });
}

// The blocks of this page that must never cover each other: it has none of the SDK's classes.
const BLOCKS = ".top h1, .lead, .filters label, .filters input, .status, .events li, #map, #detail";

test.describe("Helsinki events, the plain-HTML application (AP-14, AP-83)", () => {
  // A reader in Helsinki: the start day and its query are that calendar day.
  test.use({ timezoneId: "Europe/Helsinki", locale: "en-GB" });

  test("lists, filters, searches and details the events from its own endpoint, same origin only", async ({ page }) => {
    const requested: string[] = [];
    const reads: URL[] = [];
    const problems: string[] = [];
    page.on("request", (request) => requested.push(request.url()));
    page.on("console", (message) => {
      if (message.type() === "error") problems.push(message.text());
    });
    page.on("pageerror", (error) => problems.push(error.message));

    await serve(page, reads);

    await page.clock.setFixedTime(new Date("2026-09-22T09:00:00Z"));
    await page.goto(`${ORIGIN}/apps/helsinki-events/`);

    // AP-04: the events endpoint from `#jc-config`, not the host's primary, and only events not
    // ended before today.
    await expect(page.getByRole("status")).toHaveText("4 of 5 events");
    expect(reads).toHaveLength(1);
    expect(reads[0].pathname).toBe("/api/endpoint/events-slug/ngsi-ld/v1/entities");
    expect(reads[0].searchParams.get("type")).toBe("Event");
    expect(reads[0].searchParams.get("q")).toBe("endDate>=2026-09-21T21:00:00Z");

    const list = page.getByRole("list");
    await expect(list.getByRole("listitem")).toHaveCount(4);
    await expect(list.getByRole("button").first()).toContainText("Business plan calculations video");
    await expect(page.locator("#map circle")).toHaveCount(3);

    // An empty start date reads again, and all five are there.
    await page.getByLabel("From").fill("");
    await expect(page.getByRole("status")).toHaveText("5 of 5 events");
    expect(reads).toHaveLength(2);
    expect(reads[1].searchParams.get("q")).toBeNull();

    await page.getByLabel("To").fill("2026-10-31");
    // Everything that started by the end of October, the June market included.
    await expect(page.getByRole("status")).toHaveText("4 of 5 events");
    await page.getByLabel("To").fill("");

    await page.getByRole("searchbox", { name: "Search" }).fill("cafe");
    await expect(page.getByRole("status")).toHaveText("1 of 5 events");
    await list.getByRole("button", { name: /Café concert/ }).click();
    const detail = page.locator("#detail");
    await expect(detail).toBeFocused();
    await expect(detail.getByRole("heading", { level: 2 })).toHaveText("Café concert");
    await expect(detail).toContainText("Töölönlahdenkatu 4, Helsinki");
    await expect(list.getByRole("button", { name: /Café concert/ })).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator("#map circle.selected")).toHaveCount(1);

    // A source that is not an https link is never rendered as one.
    await page.getByRole("searchbox", { name: "Search" }).fill("summer market");
    await list.getByRole("button", { name: /Summer market/ }).click();
    await expect(detail.getByRole("heading", { level: 2 })).toHaveText("Summer market");
    await expect(detail.getByRole("link")).toHaveCount(0);

    await page.getByRole("searchbox", { name: "Search" }).fill("workshop");
    await list.getByRole("button", { name: /Workshop for Families/ }).click();
    await expect(detail.getByRole("link", { name: "Source register" })).toHaveAttribute(
      "href",
      "https://api.hel.fi/linkedevents/v1/",
    );

    // AP-12: nothing the page does trips its own policy, and it passes axe.
    expect(await axeViolations(page)).toEqual([]);
    expect(problems).toEqual([]);
    expect(requested.filter((url) => !url.startsWith(`${ORIGIN}/`))).toEqual([]);
  });

  test("says what went wrong when the endpoint refuses or the page was not served by the platform", async ({ page }) => {
    await page.route("**/*", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.startsWith("/api/endpoint/")) {
        return route.fulfill({ status: 403, contentType: "application/json", json: { title: "Forbidden" } });
      }
      const file = url.pathname.replace(/^\/apps\/helsinki-events\//, "") || "index.html";
      if (!/^[a-z]+\.(html|js|css)$/.test(file)) {
        return route.fulfill({ status: 404, body: "not found" });
      }
      let body = readFileSync(join(APP, file), "utf8");
      if (file === "index.html" && url.searchParams.has("served")) {
        body = body.replace("<head>", `<head><script id="jc-config" type="application/json">${JSON.stringify(CONFIG)}</script>`);
      }
      return route.fulfill({ status: 200, headers: { "content-type": TYPES[file.split(".").pop() ?? ""] }, body });
    });

    await page.goto(`${ORIGIN}/apps/helsinki-events/index.html?served`);
    await expect(page.getByRole("status")).toHaveText("The events could not be read (HTTP 403).");
    await expect(page.getByRole("list").getByRole("listitem")).toHaveCount(0);

    await page.goto(`${ORIGIN}/apps/helsinki-events/index.html`);
    await expect(page.getByRole("status")).toHaveText(/has no endpoint to read/);
  });

  // UI-84, SDK-12 (T-2825): the list and a chosen event at a phone, a tablet, a laptop and a
  // wall: no sideways scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
  for (const view of [
    { name: "the list", choose: null },
    { name: "a chosen event", choose: /Workshop for Families/ },
  ]) {
    for (const size of WIDTHS) {
      test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        const problems: string[] = [];
        page.on("pageerror", (error) => problems.push(error.message));
        await serve(page);
        await page.clock.setFixedTime(new Date("2026-09-22T09:00:00Z"));
        await page.setViewportSize(size);
        await page.goto(`${ORIGIN}/apps/helsinki-events/`);
        await expect(page.getByRole("status")).toHaveText("4 of 5 events");
        if (view.choose) {
          await page.getByRole("list").getByRole("button", { name: view.choose }).click();
          await expect(page.locator("#detail").getByRole("heading", { level: 2 })).toHaveText("Workshop for Families");
        }
        await testInfo.attach(`helsinki-events-${size.width}.png`, {
          body: await page.screenshot({ fullPage: true }),
          contentType: "image/png",
        });
        expect(await layoutProblems(page, BLOCKS)).toEqual([]);
        expect(problems).toEqual([]);
      });
    }
  }
});
