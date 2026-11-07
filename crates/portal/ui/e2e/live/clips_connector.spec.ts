/**
 * T-6454, journey 1: sign in, create an AI connector over one project, copy its setup line. The
 * connector is read only, lives a day and is revoked in `finally`, so a run leaves no live grant.
 * The setup holds an address and no secret: the journey asserts that on every screen it records.
 */
import { expect, test } from "@playwright/test";
import { EDITOR, csrf } from "./portal";
import { SIZES, clipOf, noSecretOnScreen } from "./clips";

test.setTimeout(240_000);

for (const size of SIZES) {
  test(`AI connector at ${size.width}`, async ({ browser }) => {
    const name = `t6454-clip-${size.width}`;
    const clip = await clipOf(browser, EDITOR, "connector", size, "/ai-connectors?lang=en", size.width === 1440);
    const { page, context } = clip;
    await context.grantPermissions(["clipboard-write"]);
    const headers = { "x-csrf-token": await csrf(context) };
    try {
      await clip.step("My AI connectors: what a connector is and the button to make one", async () => {
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
      });
      const dialog = page.getByRole("dialog");
      await clip.step("New AI connector: name it and pick what it may reach", async () => {
        await page.getByRole("button", { name: "New AI connector" }).first().click();
        await dialog.getByRole("textbox", { name: /Name/ }).fill(name);
        await dialog.getByRole("checkbox", { name: /^helsinki: the whole project/ }).check();
      });
      await clip.step("Pick the AI client that will use it", async () => {
        await dialog.getByRole("checkbox", { name: /Claude Code, VS Code/ }).check();
      });
      const card = page.getByRole("region", { name });
      await clip.step("Create: the connector exists and shows its setup line", async () => {
        await dialog.getByRole("button", { name: "Create" }).click();
        await expect(card.getByText(/claude mcp add --transport http/)).toBeVisible({ timeout: 30_000 });
      });
      await noSecretOnScreen(page);
      await clip.step("Copy the setup: an address, no secret on screen", async () => {
        await card.getByRole("button", { name: "Copy: Claude Code" }).click();
      });
      await noSecretOnScreen(page);
    } finally {
      const answer = await page.request.get("/api/v1/connectors");
      const listed = answer.ok() ? (((await answer.json()) as { items?: { id: string; name: string }[] }).items ?? []) : [];
      for (const one of listed.filter((it) => it.name === name)) await page.request.delete(`/api/v1/connectors/${one.id}`, { headers });
      await clip.finish();
    }
  });
}
