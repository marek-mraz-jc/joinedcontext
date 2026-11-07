import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-6105 (owner): a create form opens on its first field. No paragraph on the kind, no User Guide
// sentence and no "Approved as soon as you propose it" line above it; the guide is a "?" beside
// the title. T-6163: nor the strict chip or a "Fill this from a sentence" line. `vite preview` has no Portal API behind it, so the API is answered in the browser.

const PROJECT = "banskabystrica";
const EVIDENCE = process.env.JC_EVIDENCE_DIR;

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "a1", username: "jana", name: "Jana Kováčová", email: "jana@example.test", roles: ["portal-admin"] });
    }
    if (path.endsWith("/branding")) return json({ documentationBaseUrl: "https://docs.example.test" });
    // An administrator of every kind: before T-6105 each form said it is approved on propose.
    if (path.endsWith("/permissions/me")) {
      return json({
        project: PROJECT,
        bootstrap: false,
        grants: [
          {
            binding: "b",
            role: "admin",
            scope: `project:${PROJECT}`,
            rule: { kinds: ["DataSource", "Endpoint", "Pipeline", "MqttBroker"], verbs: ["read", "propose", "approve", "delete"] },
          },
        ],
      });
    }
    // The form saves its shared draft as it is typed (AG-61); there is none to load yet.
    if (path.includes("/drafts/")) {
      const manifest = (route.request().postDataJSON() as { manifest?: unknown } | null)?.manifest;
      if (manifest === undefined) return route.fulfill({ status: 404, contentType: "application/json", body: "{}" });
      return json({ manifest, version: 1, touchedBy: "jana", touchedKind: "person", updatedAt: "2026-10-29T20:00:00Z" });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

async function shot(page: Page, name: string): Promise<void> {
  if (!EVIDENCE) return;
  mkdirSync(EVIDENCE, { recursive: true });
  await page.screenshot({ path: join(EVIDENCE, `${name}.png`) });
}

const FORMS = [
  { plural: "datasources", name: "datasource", guide: /data sources in the User Guide/ },
  { plural: "endpoints", name: "endpoint", guide: /endpoints in the User Guide/ },
  { plural: "pipelines", name: "pipeline", guide: /pipelines in the User Guide/ },
  { plural: "mqttbrokers", name: "mqttbroker", guide: /User Guide/ },
];

for (const width of [1440, 400]) {
  test.describe(`create forms open on their fields at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    for (const form of FORMS) {
      test(`the ${form.name} form`, async ({ page }) => {
        await stubApi(page);
        await page.goto(`/projects/${PROJECT}/${form.plural}/new?lang=en`);
        const formPage = page.getByTestId("form-page");
        await expect(formPage.getByRole("textbox").first()).toBeVisible({ timeout: 20_000 });
        await shot(page, `${form.name}-new-${width}`);

        await expect(page.getByTestId("form-about")).toHaveCount(0);
        await expect(page.getByText(/^Read about .* in the User Guide$/)).toHaveCount(0);
        await expect(page.getByText(/Approved as soon as you propose it/)).toHaveCount(0);
        await expect(page.getByTestId("propose-standing")).toHaveCount(0);

        // T-6163: the first field shows in the first screen, the sentence button shares the tab
        // row, and strict validation (the normal case) shows no chip.
        await expect(formPage.getByRole("textbox").first()).toBeInViewport();
        await expect(page.getByText("strict validation")).toHaveCount(0);
        const fill = page.getByRole("button", { name: "Fill this from a sentence" });
        const formTab = page.getByRole("tab", { name: "Form" });
        const [fillBox, tabBox] = [await fill.boundingBox(), await formTab.boundingBox()];
        expect(Math.abs((fillBox?.y ?? 0) - (tabBox?.y ?? 100))).toBeLessThan(12);
        if (form.name === "endpoint") {
          // The project holds no space: the picker names where one is created.
          await expect(page.getByText(/Create one under Context Spaces/)).toBeVisible();
        }

        // The guide is one "?" in the title row, named for the page it opens.
        const guide = page.getByRole("link", { name: form.guide });
        await expect(guide).toBeVisible();
        await expect(guide).toHaveAttribute("target", "_blank");
        // The endpoint form's iCal group heading skips a level (chyby.md, outside this task).
        const violations = (await axeViolations(page)).filter(
          (v) => !(form.name === "endpoint" && v.startsWith("heading-order") && v.includes("representations-ical-heading")),
        );
        expect(violations).toEqual([]);
      });
    }
  });
}
