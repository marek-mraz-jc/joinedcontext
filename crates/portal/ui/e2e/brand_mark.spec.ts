import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-6891: an installation with no logo frames every page with the joinedcontext symbol, in the
// light and the dark theme; a configured logo still takes its place. The built bundle, the API
// answered from the browser.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-approver"] };
const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
const CATALOGUE = { total: 0, page: 1, pageSize: 20, datasets: [], facets: {}, unavailable: [] };
const LOGO = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 80 28"><rect width="80" height="28" rx="4" fill="#b91c1c"/></svg>';

async function stubApi(page: Page, signedIn: boolean, logo: boolean): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/branding/logo")) return route.fulfill({ contentType: "image/svg+xml", body: LOGO });
    if (path.endsWith("/branding")) return json({ instanceName: "joinedcontext", logo: logo ? "logo.svg" : "" });
    if (path.endsWith("/auth/me")) return signedIn ? json(IDENTITY) : json({ title: "Unauthorized", status: 401 }, 401);
    if (path === "/api/v1/catalogue") return json(CATALOGUE);
    return json(EMPTY_LIST);
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

const PAGES = [
  { name: "sign-in", path: "/login?lang=en", signedIn: false },
  { name: "shell", path: "/?lang=en", signedIn: true },
  { name: "catalogue", path: "/catalogue?lang=en", signedIn: false },
];

test.describe("the brand mark", () => {
  for (const scheme of ["light", "dark"] as const) {
    for (const width of [1440, 400]) {
      test(`frames each page with the joinedcontext symbol without a logo (${scheme}, ${width} px)`, async ({ page }) => {
        await page.emulateMedia({ colorScheme: scheme });
        await page.setViewportSize({ width, height: 800 });
        for (const p of PAGES) {
          await page.unrouteAll();
          await stubApi(page, p.signedIn, false);
          await page.goto(p.path);
          const symbol = page.getByTestId("jc-symbol").first();
          await expect(symbol).toBeVisible();
          // The indigo ring is lightened on the dark surface (contrast at least 3:1).
          const stroke = await symbol.locator("polygon[stroke='var(--portal-symbol-ring-a)']").evaluate((el) => getComputedStyle(el).stroke);
          expect(stroke).toBe(scheme === "dark" ? "rgb(140, 156, 245)" : "rgb(47, 63, 158)");
          await shot(page, `${p.name}-${scheme}-${width}`);
        }
      });
    }
  }

  test("a configured logo takes the symbol's place", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 800 });
    await stubApi(page, false, true);
    await page.goto("/login?lang=en");
    await expect(page.locator('img[src="/api/v1/branding/logo"]').first()).toBeVisible();
    await expect(page.getByTestId("jc-symbol")).toHaveCount(0);
    await shot(page, "sign-in-logo-light-1440");
  });
});
