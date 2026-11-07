import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6339, DM-153: the Policy form's attribute capability matrix, at 1440 and 400 px in English
// and Slovak. `vite preview` has no Portal API, so the API is answered in the browser.

const PROJECT = "banskabystrica";

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "air-public", namespace: PROJECT },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
    assigner: "did:web:banskabystrica.sk",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps", "retrieveTemporal"],
    information: [
      {
        entities: [{ type: "AirQualityObserved" }],
        attributes: { name: { history: true }, pm10: { read: false, history: true, metadata: ["observedAt", "unitCode"] } },
      },
    ],
  },
};

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "air", namespace: PROJECT },
  spec: {
    linkml:
      "classes:\n  AirQualityObserved:\n    attributes:\n      name: { range: string }\n      pm10: { range: float }\n      no2: { range: float }\n",
  },
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) => json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@banskabystrica.sk", roles: ["portal-editor"] });
    }
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith("/policies/air-public")) return json(POLICY);
    if (path.endsWith("/policies")) return list([POLICY]);
    if (path.endsWith("/datamodels")) return list([MODEL]);
    if (path.endsWith("/spaces")) {
      return list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "ovzdusie", namespace: PROJECT }, spec: {} }]);
    }
    return list([]);
  });
}

for (const lang of ["en", "sk"] as const) {
  for (const width of [1440, 400] as const) {
    test(`the capability matrix in ${lang} at ${width} px`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: width === 400 ? 900 : 1000 });
      await stubApi(page);
      await page.goto(`/projects/${PROJECT}/policies/air-public/edit?lang=${lang}`);
      const caption = lang === "en" ? /Attribute capabilities of AirQualityObserved/ : /Schopnosti atribútov typu AirQualityObserved/;
      const table = page.getByRole("table", { name: caption });
      if (!(await table.isVisible())) {
        await page.locator("summary", { hasText: lang === "en" ? "Entities and attributes" : "Entity a atribúty" }).first().click();
      }
      await expect(table).toBeVisible();
      await expect(table.getByRole("rowheader", { name: "no2" })).toBeVisible();
      const legend = page.getByText(lang === "en" ? "What the grant lets a caller do with each attribute" : "Čo oprávnenie dovoľuje volajúcemu s každým atribútom");
      await legend.scrollIntoViewIfNeeded();
      const field = page.locator("fieldset", { has: table }).last();
      const box = await field.boundingBox();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
      await page.mouse.move(0, 0);
      // A viewport shot with the matrix's legend just below the sticky app header.
      await field.evaluate((el) => {
        el.scrollIntoView({ block: "start" });
        window.scrollBy(0, -72);
      });
      await page.screenshot({ path: `test-results/T-6339/matrix-${lang}-${width}.png` });
      expect(box).not.toBeNull();
      if (lang === "en" && width === 1440) expect(await axeViolations(page)).toEqual([]);
      info.annotations.push({ type: "shot", description: `matrix-${lang}-${width}.png` });
    });
  }
}
