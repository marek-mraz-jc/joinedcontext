/**
 * T-2888 — the open-data catalogue on dev as a visitor and as a demo person meet it.
 *
 * 1. A demo person signs in to the catalogue through Keycloak: the catalogue's own single
 *    sign-on button, the realm's form, and back on the catalogue signed in. A failed sign-in
 *    names its reason on the login page (`.jc-sso-error`), which this journey reads out when
 *    it happens, so the failure a person meets is the one the sweep files.
 * 2. The joinedcontext look at 375, 768, 1440 and 2560 pixels: the mark with the installation's
 *    name, no stock CKAN teal on a link or the header, and no CKAN link or badge in the footer.
 *    A screenshot of each width goes into the report.
 * 3. CKAN holds no rows (T-5060): no dataset carries a DataStore table resource; the
 *    datasets that still do are written into the report by name.
 */
import { expect, test } from "@playwright/test";
import { axeViolations } from "../axe";
import { STEWARD } from "./portal";

test.setTimeout(180_000);

const CATALOGUE = (process.env.CKAN_URL ?? "https://opendata.dev.joinedcontext.com").replace(/\/$/, "");
/** CKAN's own teal and the darker shade its header used, as a browser reports them. */
const STOCK = ["rgb(32, 107, 130)", "rgb(0, 93, 122)"];
const WIDTHS = [375, 768, 1440, 2560];

test("a demo person signs in to the catalogue through Keycloak", async ({ browser }) => {
  if (!STEWARD.password) {
    throw new Error(`no password in the environment for ${STEWARD.user} (PORTAL_PASSWORD)`);
  }
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await page.goto(`${CATALOGUE}/user/login`, { waitUntil: "load" });
    await Promise.all([
      page.waitForURL(/\/realms\//, { waitUntil: "load" }),
      page.getByRole("link", { name: "Log in via single sign-on" }).click(),
    ]);
    await page.fill("#username", STEWARD.user);
    await page.fill("#password", STEWARD.password);
    await Promise.all([page.waitForURL((url) => url.origin === CATALOGUE, { waitUntil: "load" }), page.click("#kc-login")]);

    const refused = page.locator(".jc-sso-error");
    if (await refused.count()) {
      throw new Error(`the catalogue refused the sign-in: ${await refused.innerText()}`);
    }
    expect(new URL(page.url()).pathname, "back on the login page means the sign-in did not finish").not.toMatch(/^\/user\/login/);
    // Signed in: the account masthead offers the way out, which a visitor never sees.
    await expect(page.locator(".account-masthead").getByRole("button", { name: /log out/i })).toBeVisible({ timeout: 30_000 });
  } finally {
    await context.close();
  }
});

for (const width of WIDTHS) {
  test(`the catalogue wears the installation's look at ${width} px`, async ({ browser }, info) => {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    const page = await context.newPage();
    try {
      await page.goto(`${CATALOGUE}/`, { waitUntil: "load" });
      const brand = page.locator("a.jc-brand");
      await expect(brand).toBeVisible();
      await expect(brand.locator("span")).not.toHaveText("");
      await expect(brand.locator("img")).toBeVisible();

      const colours = await page.evaluate(() => {
        const read = (selector: string, property: "color" | "backgroundColor") => {
          const element = document.querySelector(selector);
          return element ? getComputedStyle(element)[property] : null;
        };
        return {
          link: read("main a, .main a, #content a", "color"),
          masthead: read(".masthead", "backgroundColor"),
          search: read(".homepage .module-search .search-form", "backgroundColor"),
        };
      });
      for (const [where, colour] of Object.entries(colours)) {
        expect(STOCK, `${where} is CKAN's stock teal`).not.toContain(colour);
      }

      const footer = page.locator("footer.site-footer");
      for (const stock of ["docs.ckan.org", "ckan.org", "opendefinition.org"]) {
        await expect(footer.locator(`a[href*="${stock}"]`), stock).toHaveCount(0);
      }
      await expect(footer.locator('img[src*="od_80x15"]')).toHaveCount(0);

      // No sideways scroll at any width: the header and the footer fit the page.
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow, "the page scrolls sideways").toBeLessThanOrEqual(0);

      await info.attach(`catalogue-${width}.png`, { body: await page.screenshot({ fullPage: false }), contentType: "image/png" });
    } finally {
      await context.close();
    }
  });
}

interface Resource {
  id: string;
  url_type?: string | null;
  datastore_active?: boolean;
}

test("no dataset carries a DataStore table", async ({ request }, info) => {
  const search = await request.get(`${CATALOGUE}/api/3/action/package_search?rows=1000`);
  expect(search.ok(), `package_search: ${search.status()}`).toBe(true);
  const datasets = ((await search.json()) as { result: { results: { name: string; resources: Resource[] }[] } }).result.results;
  expect(datasets.length, "the catalogue lists datasets").toBeGreaterThan(0);

  const tables = datasets
    .filter((dataset) => dataset.resources.some((one) => one.url_type === "datastore" || one.datastore_active === true))
    .map((dataset) => dataset.name);
  await info.attach("datastore-tables.json", { body: JSON.stringify(tables, null, 2), contentType: "application/json" });
  expect(tables, "datasets that still carry a DataStore table").toEqual([]);
});

/**
 * T-5108 (WCAG 2.1 AA, Decree 78/2020): the catalogue's pages a visitor meets first carry no
 * serious or critical axe finding at desktop and phone width, and the footer's accessibility
 * statement is reachable with the keyboard alone.
 */
const AXE_PAGES = ["/", "/dataset/", "/organization/", "/about", "/accessibility"];

for (const width of [1440, 400]) {
  test(`the catalogue's first pages have no serious axe finding at ${width} px`, async ({ browser, request }) => {
    const found = ((await (await request.get(`${CATALOGUE}/api/3/action/package_search?rows=1`)).json()) as {
      result: { results: { name: string }[] };
    }).result.results;
    const pages = [...AXE_PAGES, ...found.map((dataset) => `/dataset/${dataset.name}`)];
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    const page = await context.newPage();
    const findings: string[] = [];
    try {
      for (const path of pages) {
        const answer = await page.goto(`${CATALOGUE}${path}`, { waitUntil: "load" });
        expect(answer?.status(), path).toBeLessThan(400);
        findings.push(
          ...(await axeViolations(page)).filter((line) => /\((serious|critical)\)/.test(line)).map((line) => `${path}: ${line}`),
        );
      }
    } finally {
      await context.close();
    }
    expect(findings).toEqual([]);
  });
}

test("a keyboard-only visitor reaches the catalogue's accessibility statement from its footer", async ({ page }) => {
  await page.goto(`${CATALOGUE}/`, { waitUntil: "load" });
  const link = page.locator("footer").getByRole("link", { name: /^(Accessibility statement|Vyhlásenie o prístupnosti)$/ });
  for (let presses = 0; presses < 120; presses += 1) {
    await page.keyboard.press("Tab");
    const focused = await page.evaluate(() => document.activeElement?.getAttribute("href") ?? "");
    if (focused.endsWith("/accessibility")) break;
  }
  await expect(link).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/accessibility$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(/Accessibility statement|Vyhlásenie o prístupnosti/);
});
