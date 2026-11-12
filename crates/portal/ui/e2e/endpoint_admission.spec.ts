import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// T-6802: an administrator outside an Endpoint's project groups opens its page and
// settings/access; the Portal's admission answer says the gateway refuses them, the pages show the
// refused state and ask nothing under /api/endpoint/{slug}. Admitted, both read as before.
// `vite preview` has no Portal behind it: the API is answered in the browser.

const PROJECT = "helsinki";
const NAME = "app-air-quality";
const SLUG = "3jlwl5dg2oidblwqhc6qdi3zxl";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT, title: "Air quality app" },
  spec: { contextSpaceRef: "air", slug: SLUG, audience: "project-list", enabledRepresentations: ["ngsi-ld", "mcp"] },
  status: { phase: "Live" },
};
const ACCESS = {
  subject: { type: "user", id: "smoke-ui-reader" },
  resource: { type: "endpoint", id: SLUG, space: "air" },
  permissions: [{ resource: { type: "AirQualityObserved" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*", constraints: {} }],
  prohibitions: [],
};

/** Answers the page as the Portal and the gateway would; returns the gateway paths it was asked. */
async function stub(page: Page, admitted: boolean): Promise<string[]> {
  const gateway: string[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.startsWith("/api/endpoint/")) {
      gateway.push(path);
      if (!admitted) return json({ title: "Forbidden", status: 403 }, 403);
      if (path.endsWith("/access")) return json(ACCESS);
      if (path.endsWith("/ngsi-ld/v1/entities")) return json([]);
      return json({}, 404);
    }
    if (path.endsWith("/auth/me"))
      return json({ subject: "s", username: "smoke-ui-admin", name: "Smoke Admin", roles: ["portal-admin"] });
    if (path === "/api/v1/projects") return json({ items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith(`/endpoints/${NAME}/admission`)) return json({ admitted });
    if (path.endsWith(`/endpoints/${NAME}`)) return json(ENDPOINT);
    if (path.endsWith(`/${PROJECT}/endpoints`)) return json(list([ENDPOINT]));
    if (path.endsWith(`/endpoints/${NAME}/access-requests`)) return json({ types: ["AirQualityObserved"], refusal: null, items: [] });
    return json(list([]));
  });
  return gateway;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("an Endpoint the gateway does not admit you to (T-6802)", () => {
  for (const width of [1440, 400]) {
    test(`the refused state, no gateway read, at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const gateway = await stub(page, false);
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      const refused = page.locator("[data-endpoint-refused]");
      await expect(refused.getByText("You do not read this Endpoint's data")).toBeVisible();
      await refused.scrollIntoViewIfNeeded();
      await shot(page, `endpoint-refused-${width}`);
      await refused.getByRole("link", { name: "Request access" }).click();
      await expect(page).toHaveURL(/#request-access$/);

      await page.goto(`/projects/${PROJECT}/settings/access?lang=en`);
      await expect(page.locator("[data-endpoint-refused]").getByText("You do not read this Endpoint's data")).toBeVisible();
      await shot(page, `settings-access-refused-${width}`);
      await page.locator("[data-endpoint-refused]").getByRole("link", { name: "Request access" }).click();
      await expect(page).toHaveURL(new RegExp(`/endpoints/${NAME}#request-access$`));
      expect(gateway).toEqual([]);
    });

    test(`an admitted reader reads as before at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const gateway = await stub(page, true);
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      await expect.poll(() => gateway.length).toBeGreaterThan(0);
      await expect(page.locator("[data-endpoint-refused]")).toHaveCount(0);
      await shot(page, `endpoint-granted-${width}`);

      await page.goto(`/projects/${PROJECT}/settings/access?lang=en`);
      await expect.poll(() => gateway).toContain(`/api/endpoint/${SLUG}/access`);
      await expect(page.locator("[data-endpoint-refused]")).toHaveCount(0);
      await shot(page, `settings-access-granted-${width}`);
    });
  }
});
