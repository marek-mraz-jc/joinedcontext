/**
 * T-3219 — importing an App from its export, as a person who has never done it (UI-87, PF-57):
 * a steward exports one of helsinki's Apps, opens Organization → Applications → Import app, and
 * meets every field with what it needs (the project it joins, the archive, an optional name),
 * checks the archive and reads the repository the import would create and the red lane it lands
 * in. Nothing is imported: the journey stops at the check, which creates nothing (PF-57).
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";

test.setTimeout(180_000);

test("a steward checks an App's export as an import and reads what it would create", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/organization/applications?lang=en`);
  try {
    const listed = await page.request.get(`/api/v1/projects/${PROJECT}/apps`);
    expect(listed.ok(), `list apps: ${listed.status()}`).toBe(true);
    const { items } = (await listed.json()) as { items: { metadata: { name: string } }[] };
    expect(items.length, "helsinki has an App to export").toBeGreaterThan(0);
    const app = items[0].metadata.name;
    const exported = await page.request.get(`/api/v1/projects/${PROJECT}/apps/${app}/export`);
    expect(exported.status(), `export ${app}`).toBe(200);
    const archive = await exported.body();

    await page.getByRole("button", { name: "Import app" }).click();
    const dialog = page.getByRole("dialog", { name: "Import an App" });
    // What each field needs is on the screen before anything is wrong.
    await expect(dialog.getByLabel(/^Project/)).toHaveAccessibleDescription("The project the App joins.");
    await expect(dialog.getByText("A .zip written by Export on this tab.")).toBeVisible();
    const check = dialog.getByRole("button", { name: "Check the archive" });
    await expect(check).toHaveAttribute("aria-disabled", "true");

    await dialog.getByLabel(/^Project/).selectOption(PROJECT);
    await dialog.locator('input[type="file"]').setInputFiles({ name: `${app}.zip`, mimeType: "application/zip", buffer: archive });
    await dialog.getByLabel(/^Name in the project/).fill(`${app}-import-check`.slice(0, 63));
    await check.click();
    await expect(dialog.getByRole("heading", { name: "Repository to create" })).toBeVisible({ timeout: 60_000 });
    await expect(dialog.getByText("Import proposes the App as a red-lane change")).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveCount(0);
  } finally {
    await context.close();
  }
});
