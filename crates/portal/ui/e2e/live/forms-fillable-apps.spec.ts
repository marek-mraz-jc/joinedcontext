/**
 * Every form fillable by a new user: Apps (T-3214).
 *
 * A person who has never seen the platform opens Apps, starts "Generate your own app" and fills
 * the builder with only what the screen gives them: the grey button says what it still waits
 * for, the endpoint is a picker over what the project publishes, the preview shows what that
 * endpoint holds before a word is written, and the name is derived. It stops at the ready
 * button: pressing it spends the model, and that end of the workflow (build, publish, open) is
 * walked nightly by `readiness-app.spec.ts`.
 */
import { expect, test } from "@playwright/test";
import { EDITOR, STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";

for (const who of [STEWARD, EDITOR]) {
  test(`a new user fills the app builder from the screen alone (${who.user})`, async ({ browser }) => {
    const { context, page } = await signIn(browser, who, `/projects/${PROJECT}/apps?lang=en`);
    try {
      const start = page.getByRole("button", { name: "Generate your own app" });
      if ((await start.getAttribute("aria-disabled")) === "true") {
        // A role that may not propose an App is told so on the button, before typing anything.
        await expect(start).toHaveAccessibleDescription(/propose/);
        return;
      }
      await start.click();
      const builder = page.getByTestId("assistant-build");
      await expect(builder).toBeVisible({ timeout: 60_000 });

      const submit = builder.getByRole("button", { name: "Generate the app" });
      await expect(submit).toHaveAccessibleDescription("Choose the endpoint the app reads first.");

      // The endpoint is a picker over what the project publishes, never a typed address.
      const endpoint = builder.getByLabel("Endpoint", { exact: false }).first();
      const options = endpoint.locator("option:not([value=''])");
      await expect(options.first()).toBeAttached({ timeout: 30_000 });
      await endpoint.selectOption({ index: 1 });
      await expect(builder.getByText("What this endpoint gives you")).toBeVisible({ timeout: 30_000 });
      await expect(submit).toHaveAccessibleDescription("Describe what the app should do.");

      await builder.getByLabel("What should the app do?").fill("A table of what this endpoint publishes, newest first.");
      // The name is derived from the description: nothing to invent, and the button opens.
      await expect(submit).not.toHaveAttribute("aria-disabled", "true");
      await expect(submit).toBeEnabled();
    } finally {
      await context.close();
    }
  });
}
