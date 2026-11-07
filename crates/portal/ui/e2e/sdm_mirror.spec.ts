// T-6353: a Smart Data Model taken from the mirror, in the new-model window: search, pick, the
// models that did not convert named with why, then the import renamed in the editor before Save.
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const PROJECT = "banskabystrica";
const IDENTITY = { subject: "s", username: "jana", name: "Jana", email: "jana@bb.sk", roles: ["portal-editor"] };
const COMMIT = "9f1c2b7d4e6a8c0b2d4f6a8c0e2b4d6f8a0c2e4b";
const LINKML = [
  "id: https://github.com/smart-data-models/dataModel.Environment/AirQualityObserved",
  "name: AirQualityObserved",
  "annotations:",
  "  spec.source.repository: https://github.com/smart-data-models/dataModel.Environment",
  "  spec.source.path: AirQualityObserved",
  `  spec.source.commit: ${COMMIT}`,
  "classes:",
  "  AirQualityObserved:",
  "    slots: [pm10, airQualityLevel]",
  "slots:",
  "  pm10: { range: float }",
  "  airQualityLevel: { range: AirQualityLevel }",
  "enums:",
  "  AirQualityLevel:",
  "    permissible_values:",
  "      good: {}",
  "      poor: {}",
  "",
].join("\n");
const CATALOGUE = {
  refreshedAt: "2026-11-02T03:17:00Z",
  subjects: [
    {
      name: "dataModel.Environment",
      title: "Environment",
      models: [
        { id: "dataModel.Environment/AirQualityObserved", name: "AirQualityObserved", description: "Air quality observed at a station", attributes: ["pm10", "airQualityLevel"], commit: COMMIT },
        { id: "dataModel.Environment/NoiseLevelObserved", name: "NoiseLevelObserved", description: "Noise levels over a period", attributes: ["LAeq"], commit: COMMIT },
      ],
    },
  ],
  failed: [{ id: "dataModel.Agrifood/Animal", finding: "slots.weight: primitive-range: weight on Animal is a Relationship with the range float" }],
};

async function stub(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith("/sdm-catalog")) return json(CATALOGUE);
    if (path.endsWith("/import-sdm")) return json({ linkml: LINKML });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const shot = (page: Page, name: string) => page.screenshot({ path: `test-results/T-6353/${name}.png`, fullPage: false });

for (const [width, height] of [[1440, 900], [400, 860]] as const) {
  test(`a model from the mirror is searched, picked and renamed before Save at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stub(page);
    await page.goto(`/projects/${PROJECT}/models?new=sdm&lang=en`);
    const naming = page.getByRole("dialog", { name: "New data model" });
    await naming.getByRole("textbox", { name: "Name", exact: true }).fill("Air");
    await naming.getByRole("button", { name: "Create model" }).click();
    await page.getByRole("button", { name: "Add type", exact: true }).click();
    await page.getByRole("dialog", { name: "Add type" }).getByRole("button", { name: /Import from Smart Data Models/ }).click();

    await page.getByRole("searchbox").or(page.getByPlaceholder(/search/i)).first().fill("air");
    await expect(page.getByRole("button", { name: /AirQualityObserved/ }).first()).toBeVisible();
    await expect(page.getByRole("button", { name: /NoiseLevelObserved/ })).toHaveCount(0);
    await shot(page, `search-${width}`);

    await page.getByText("Models that could not be converted: 1").click();
    await expect(page.getByText("primitive-range")).toBeVisible();
    await shot(page, `failed-${width}`);

    await page.getByRole("button", { name: /AirQualityObserved/ }).first().click();
    await expect(page.getByText(COMMIT.slice(0, 7)).first()).toBeVisible();
    await shot(page, `picked-${width}`);
    await page.getByRole("button", { name: /^(Add|Use|Import)/ }).last().click();

    const renamed = page.getByRole("textbox", { name: "Type name" });
    await expect(renamed).toHaveValue("AirQualityObserved");
    await shot(page, `imported-${width}`);
    await renamed.fill("AirReading");
    await renamed.press("Enter");
    await expect(page.getByRole("textbox", { name: "Type name" })).toHaveValue("AirReading");
    await shot(page, `renamed-${width}`);
  });
}
