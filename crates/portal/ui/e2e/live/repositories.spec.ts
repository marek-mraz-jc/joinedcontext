// The project's repositories on dev (jc-shako-ui): a viewer opens the list from the address, and
// the first repository it lists, read through the registry over shako. Read-only; whatever the
// forge holds, the page answers with a heading and a list, an empty state or a named refusal.
import { expect, test } from "@playwright/test";
import { VIEWER, goSignedIn } from "./portal";

test("a viewer opens the project's repositories and the first one, with no settings tab", async ({ page }) => {
  await goSignedIn(page, VIEWER, "/projects/helsinki/repositories?lang=en");
  await expect(page.getByRole("heading", { level: 1, name: "Repositories" })).toBeVisible({ timeout: 60_000 });
  await expect(page.getByText("Loading from the forge")).toHaveCount(0, { timeout: 60_000 });
  const first = page.getByRole("main").getByRole("table").getByRole("link").first();
  if ((await first.count()) === 0) return;
  const name = (await first.textContent()) ?? "";
  await first.click();
  await expect(page).toHaveURL(/\/projects\/helsinki\/repositories\/[^/?]+/);
  await expect(page.getByRole("heading", { level: 1, name })).toBeVisible({ timeout: 60_000 });
  await expect(page.getByRole("tab", { name: "Files" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Settings" })).toHaveCount(0);
});

test("a repository the project does not own is not there: a named refusal, never its files", async ({ page }) => {
  await goSignedIn(page, VIEWER, "/projects/helsinki/repositories/no-such-repository?lang=en");
  await expect(page.getByRole("heading", { level: 1, name: "no-such-repository" })).toBeVisible({ timeout: 60_000 });
  await expect(page.getByRole("alert").first()).toBeVisible({ timeout: 60_000 });
  await expect(page.getByRole("tabpanel").getByRole("table")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "All repositories" })).toHaveAttribute("href", "/projects/helsinki/repositories");
});
