import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

/**
 * The secret form on dev (T-5971, MF-24): the page a plan's needsSecret links to opens for a
 * steward with the name and key prefilled and a hint under each field. Nothing is submitted: a
 * submit opens a change request on the forge.
 */
test("the secret form a plan links to opens prefilled with its hints", async ({ browser }) => {
  const { page } = await signIn(browser, STEWARD, `/projects/helsinki/secrets/new?name=counters-api&key=apiKey&lang=en`);
  await expect(page.getByRole("heading", { name: "Enter a secret" })).toBeVisible();
  await expect(page.getByLabel(/^Secret name/)).toHaveValue("counters-api");
  await expect(page.getByText(/never in a chat/)).toBeVisible();
  await expect(page.getByLabel(/^Value/)).toHaveAttribute("type", "password");
});
