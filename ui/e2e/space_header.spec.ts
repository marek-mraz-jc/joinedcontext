import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// The space page's header (T-6092, T-6138, owner 2026-10-31): the name, the description behind the
// info mark beside it (the purpose line the live walker reads), the state and the actions on one row.
// Against a stubbed API; shots at 1440 and 400 go to test-results/space-header.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@example.org", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LONG =
  "Air quality stations of the city and what they measured every hour: PM10, PM2.5, nitrogen dioxide and ozone, " +
  "kept for five years and published to the open data catalogue for anyone who wants to compare the districts.";
const space = (description: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "ovzdusie", namespace: "city", title: { en: "Air quality" }, description: { en: description } },
  spec: {},
  status: { phase: "Ready" },
});

async function stubApi(page: Page, description: string): Promise<void> {
  const SPACE = space(description);
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "city" }] });
    if (path === "/api/v1/projects/city/spaces/ovzdusie") return json(SPACE);
    if (path.endsWith("/spaces")) return json(list([SPACE]));
    return json(list([]));
  });
}

const SHOTS = join(process.cwd(), "test-results", "space-header");

for (const width of [1440, 400]) {
  test.describe(`space header at ${width}`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("keeps the name, the purpose behind its mark and the state on one row, no copy chip (owner 2026-10-31)", async ({ page }) => {
      await stubApi(page, LONG);
      await page.goto("/projects/city/spaces/ovzdusie?lang=en");
      const h1 = page.getByRole("heading", { level: 1, name: "Air quality" });
      await expect(h1).toBeVisible();
      await expect(page.getByRole("button", { name: "ovzdusie" })).toHaveCount(0);
      // The purpose is in the document for a screen reader but takes no room (sr-only) until the mark is asked.
      const purpose = page.getByRole("tooltip").filter({ hasText: LONG });
      await expect(purpose).toHaveClass(/sr-only/);
      expect((await purpose.boundingBox())!.height).toBeLessThanOrEqual(1);
      mkdirSync(SHOTS, { recursive: true });
      await page.screenshot({ path: join(SHOTS, `1-header-${width}.png`) });
    });
  });
}
