/**
 * Every form fillable by a new user: CKAN catalogue publishing (T-3215).
 *
 * A person new to the platform opens Open data on dev's `helsinki` (its seeded catalogue, T-2407)
 * and walks the three workflows with only what the screen gives them: registering a catalogue
 * (a pasted token is refused at the field, before anything is proposed), publishing an endpoint as
 * a dataset (the button on this page starts the endpoint page's one-step flow, which drafts the
 * metadata from the model and the organization), and opening a published dataset in CKAN.
 * Nothing is proposed: the draft is a read, and the flow is closed before "Propose publication".
 */
import { expect, test } from "@playwright/test";
import { guideShot } from "./guide";
import { EDITOR, STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";
const PAGE = `/projects/${PROJECT}/ckan?lang=en`;

for (const who of [STEWARD, EDITOR]) {
  test(`a new user registers, publishes and opens from the Open data page alone (${who.user})`, async ({ browser }) => {
    const { context, page } = await signIn(browser, who, PAGE);
    try {
      await expect(page.getByRole("heading", { level: 1, name: "Open-data catalogue" })).toBeVisible({ timeout: 60_000 });
      // Before anything is typed: step 1 types a token-shaped string no shot may show.
      if (who === STEWARD) await guideShot(page, "ckan-1-open-data-page", { reloads: true });

      // 1. Register a catalogue: the token itself never goes into a field.
      const propose = page.getByRole("button", { name: "Propose catalogue" });
      if ((await propose.getAttribute("aria-disabled")) !== "true") {
        const proposals: string[] = [];
        page.on("request", (request) => {
          if (request.method() === "POST" && request.url().includes("/ckaninstances")) proposals.push(request.url());
        });
        await page.getByLabel(/^Name/).fill("new-user-check");
        await page.getByLabel(/^URL/).fill("https://opendata.example.org");
        await page.getByLabel("API token secret").fill("eyJhbGciOiJIUzI1NiJ9.eyJqdGkiOiJ4In0.c2ln");
        await expect(page.getByLabel("Key inside the secret")).toHaveValue("apiToken");
        await propose.click();
        await expect(page.getByText(/looks like the API token itself/)).toBeVisible();
        await expect(page.getByLabel("API token secret")).toBeFocused();
        expect(proposals, "a pasted token was proposed").toEqual([]);
      } else {
        await expect(propose).toHaveAccessibleDescription(/propose/);
      }

      // 2. Publish an endpoint as a dataset, from this page.
      const publish = page.getByRole("button", { name: "Publish a dataset" });
      if ((await publish.getAttribute("aria-disabled")) !== "true") {
        await publish.click();
        const dialog = page.getByRole("dialog", { name: "Publish a dataset" });
        await expect(dialog).toBeVisible();
        const endpoint = dialog.getByLabel("Endpoint", { exact: false }).first();
        await expect(endpoint.locator("option:not([value=''])").first()).toBeAttached({ timeout: 30_000 });
        await endpoint.selectOption({ index: 1 });
        await dialog.getByRole("button", { name: "Draft the description" }).click();
        await expect(dialog.getByText(/^Drafted from endpoint/)).toBeVisible({ timeout: 30_000 });
        await expect(dialog.getByRole("button", { name: "Preview the entry" })).toBeVisible();
        await dialog.getByRole("button", { name: "Close" }).first().click();
      }

      // 3. Open a published dataset in CKAN.
      const published = page.getByRole("region", { name: "Published endpoints" });
      const dataset = published.getByRole("link").first();
      await expect(dataset, "helsinki publishes at least one dataset with an address").toBeVisible();
      const href = (await dataset.getAttribute("href")) ?? "";
      const answer = await page.request.get(href);
      expect(answer.ok(), `${href}: ${answer.status()}`).toBe(true);
    } finally {
      await context.close();
    }
  });
}
