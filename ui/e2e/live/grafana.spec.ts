import { expect, test } from "@playwright/test";

import { EDITOR, goSignedIn } from "./portal";

const ADDRESS = "/projects/helsinki/grafana";

test.describe("Grafana visualizations", () => {
  // The whole way when the project's Grafana runs. Without it the page must say whom to ask: dev
  // gets Grafana with T-6513, and T-6516 films this journey there.
  test("a person saves an Endpoint's data into Grafana and removes it", async ({ page }) => {
    test.setTimeout(120_000);
    await goSignedIn(page, EDITOR, ADDRESS);
    await expect(page.getByRole("heading", { level: 1, name: "Grafana" })).toBeVisible();
    const form = page.getByRole("button", { name: "Save to Grafana" });
    const notReady = page.getByText("Grafana is not ready for this project");
    await expect(form.or(notReady)).toBeVisible();
    if (await notReady.isVisible()) return;

    const title = `t6509 ${Date.now()}`;
    await page.getByLabel("Name", { exact: true }).fill(title);
    const endpoint = page.getByLabel("Endpoint", { exact: true });
    await expect(endpoint.locator("option").nth(1)).toBeAttached();
    await endpoint.selectOption({ index: 1 });
    const type = page.getByLabel("Type", { exact: true });
    await expect(type.locator("option").nth(1)).toBeAttached();
    await type.selectOption({ index: 1 });
    const attribute = page.getByLabel("Attribute", { exact: true });
    await expect(attribute.locator("option").nth(1)).toBeAttached();
    await attribute.selectOption({ index: 1 });
    await form.click();

    const saved = page.getByRole("listitem").filter({ hasText: title });
    await expect(saved).toBeVisible();
    await expect(saved.getByRole("link", { name: `Open ${title} in Grafana` })).toHaveAttribute("href", /\/d\/pv-/);
    await saved.getByRole("button", { name: `Remove ${title}` }).click();
    await saved.getByRole("button", { name: "Remove from Grafana" }).click();
    await expect(saved).toHaveCount(0);
  });
});
