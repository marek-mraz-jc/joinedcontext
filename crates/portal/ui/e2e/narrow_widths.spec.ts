import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-4265 (photo audit T-4160 #0, #41, #42): the Portal at a tablet's and a phone's width. `vite
// preview` has no Portal API behind it, so the API is answered in the browser.

const PROJECT = "bbsk";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const space = (name: string, title: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name, namespace: PROJECT, title: { sk: title, en: title } },
  spec: { dataModelRef: "key-performance-indicator" },
  status: { phase: "Live" },
});

const SPACES = [
  space("bbsk-kpi", "Ukazovatele kraja"),
  space("bbsk-kraj", "Kraj — zdrojové údaje"),
  space("bbsk-registre", "Kraj — registre"),
];

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) => json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    if (path === `/api/v1/projects/${PROJECT}/spaces`) return list(SPACES);
    return list([]);
  });
}

/** How far an element's content reaches past its own box, in CSS px. */
const overflowOf = (page: Page, selector: string) =>
  page.locator(selector).first().evaluate((el) => el.scrollWidth - el.clientWidth);

test.describe("at 768 px", () => {
  test.use({ viewport: { width: 768, height: 1024 } });

  test("the sidebar folds behind the menu button and the spaces table fits the page", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces?lang=sk`);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.getByRole("button", { name: "Menu" })).toBeVisible();
    await expect(page.locator("#portal-sidebar")).toBeHidden();
    const frame = page.getByRole("group", { name: "Kontextové priestory" });
    // The row's name opens the space (T-6095: no "Look inside" button beside the menu).
    await expect(frame.getByRole("link", { name: "Ukazovatele kraja" })).toBeVisible();
    expect(await frame.evaluate((el) => el.scrollWidth - el.clientWidth), "the table is wider than its frame").toBeLessThanOrEqual(1);

    // The menu button still opens the whole navigation.
    await page.getByRole("button", { name: "Menu" }).click();
    await expect(page.getByRole("navigation", { name: "Hlavná navigácia" })).toBeVisible();
  });

});

// A form's columns follow its card, not the screen: a nested group is narrower than its form, and
// two columns in it cut each value to a few letters (T-4265, photo audit T-4160 #42, #43).
for (const [width, docked] of [[768, false], [1024, false], [1280, true]] as const) {
  for (const kind of ["spaces", "assistantdeployments"]) {
    test(`the ${kind} create form at ${width} px${docked ? " beside the assistant" : ""} gives every field room for its value`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1024 });
      await stubApi(page);
      await page.goto(`/projects/${PROJECT}/${kind}/new?lang=sk`);
      const form = page.locator('[data-testid="form-page"] form').first();
      await expect(form.getByRole("textbox").first()).toBeVisible();
      if (docked) {
        // The assistant docks beside the page from lg up and takes its width from the form.
        await page.locator("[data-assistant-bubble]").click();
        await page.waitForTimeout(500);
      }
      const narrow = await form.evaluate((el) =>
        [...el.querySelectorAll<HTMLElement>("input:not([type=checkbox]):not([type=radio]):not([type=hidden]):not([type=file]), select, textarea")]
          .filter((field) => field.offsetParent !== null)
          .map((field) => ({ name: field.getAttribute("name") ?? field.id, width: Math.round(field.getBoundingClientRect().width) }))
          .filter((field) => field.width < 200),
      );
      expect(narrow, "fields narrower than 200 px").toEqual([]);
      expect(await overflowOf(page, '[data-testid="form-page"]'), "the form runs past its page").toBeLessThanOrEqual(1);
    });
  }
}

test.describe("at 375 px", () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test("a space's state and its way inside never cover each other, and a table that scrolls says so", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces?lang=sk`);
    const frame = page.getByRole("group", { name: "Kontextové priestory" });
    const rows = frame.locator("tbody tr");
    await expect(rows).toHaveCount(SPACES.length);
    for (let i = 0; i < SPACES.length; i++) {
      const state = await rows.nth(i).getByText("Aktívne").boundingBox();
      // The row's name is its way inside (T-6095).
      const open = await rows.nth(i).locator("a[data-row-link]").boundingBox();
      expect(state && open, "the state and the link are laid out").toBeTruthy();
      if (!state || !open) return;
      const apart = state.x + state.width <= open.x || open.x + open.width <= state.x || state.y + state.height <= open.y || open.y + open.height <= state.y;
      expect(apart, `row ${i + 1}: the state badge and the space's name overlap`).toBe(true);
    }
    // The frame's edge carries a shadow while columns wait beyond it.
    const shadow = await frame.evaluate((el) => getComputedStyle(el).backgroundImage);
    expect(shadow).toContain("gradient");
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(sideways, "the page scrolls sideways").toBeLessThanOrEqual(0);
  });
});

// T-4991: the data sources list shows each address as host and path, the query on hover, and
// the secret an address template reads by name, never the template.
for (const width of [400, 1440]) {
  test(`the data sources list at ${width} px shows short addresses and fits the page`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const source = (name: string, spec: Record<string, unknown>) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name, namespace: PROJECT },
      spec,
    });
    await stubApi(page);
    await page.route(`**/api/v1/projects/${PROJECT}/datasources`, (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [
            source("demo-counters", { type: "nats", input: { urls: ["nats://demo:${DEMO_FEED_PASSWORD}@demo-nats.dev.svc.cluster.local:4222"] } }),
            source("hel-districts", {
              type: "http",
              http: { url: "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&version=2.0.0&request=GetFeature&outputFormat=application/json&typeNames=avoindata:Kaupunginosajako" },
            }),
          ],
        }),
      }),
    );
    await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
    await expect(page.getByText("kartta.hel.fi/ws/geoserver/avoindata/wfs", { exact: true })).toHaveAttribute("title", /\?service=WFS/);
    await expect(page.getByText("secret from DEMO_FEED_PASSWORD")).toBeVisible();
    await expect(page.getByText("${DEMO_FEED_PASSWORD}", { exact: false })).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(0);
    await page.screenshot({ path: test.info().outputPath(`datasources-${width}.png`), fullPage: true });
  });
}
