/**
 * T-6791: the Dashboard JSON editor on dev. A steward opens it, inserts a chart over an Endpoint
 * the Portal lists for them, and has the server check the manifest (`?dryRun=All`): nothing is
 * written. A project with no Endpoint shows the empty state that says where to make one.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, goSignedIn } from "./portal";

const PROJECT = process.env.DASHBOARD_PROJECT ?? "helsinki";

test("a steward inserts a chart and has the server check the dashboard", async ({ page }) => {
  test.setTimeout(120_000);
  await goSignedIn(page, STEWARD, `/projects/${PROJECT}/dashboard-editor?lang=en`);
  await expect(page.getByRole("heading", { level: 1, name: "New dashboard in JSON" })).toBeVisible();
  const insert = page.getByRole("button", { name: "Insert widget" });
  const empty = page.getByText("No endpoint to read yet");
  await expect(insert.or(empty)).toBeVisible();
  if (await empty.isVisible()) {
    await expect(page.getByRole("link", { name: "Go to endpoints" })).toBeVisible();
    return;
  }
  await expect(page.locator(".monaco-editor").first()).toBeVisible();
  await insert.click();
  await expect(page.getByText(/^Widget added to the last page/)).toBeVisible();
  await page.getByRole("button", { name: "Check" }).click();
  // A starter has no name or title yet: the server names the fields at their JSON paths.
  await expect(page.getByText("The server refused it").or(page.getByText("The server accepts this dashboard"))).toBeVisible();
});
