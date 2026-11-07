/**
 * The secret form in a browser (T-5971, MF-24): the page a plan's needsSecret links to, at a
 * desktop and a phone width, empty and after a refusal. The API is answered in the browser.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";

async function stubApi(page: Page, refuse: boolean): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path === `/api/v1/projects/${PROJECT}/secrets` && request.method() === "POST") {
      return json(
        refuse
          ? { type: "https://joinedcontext.com/errors/unprocessable", title: "Unprocessable Content", status: 422,
              detail: "the secret cannot be written as sent; nothing was kept",
              errors: [{ path: "values.apiKey", message: "1 to 16384 bytes" }] }
          : { apiVersion: API, kind: "Change", metadata: { name: "pr-7", project: PROJECT },
              status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } } },
        refuse ? 422 : 202,
      );
    }
    if (path === `/api/v1/projects/${PROJECT}/secrets`) return json({ items: [] });
    if (path === `/api/v1/projects/${PROJECT}`) {
      return json({ apiVersion: API, kind: "Project", metadata: { name: PROJECT }, spec: {}, status: {} });
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("the form takes the key a plan asked for and shows the change to approve", async ({ page }) => {
      await stubApi(page, false);
      await page.goto(`/projects/${PROJECT}/secrets/new?name=counters-api&key=apiKey&lang=en`);
      await expect(page.getByRole("heading", { name: "Enter a secret" })).toBeVisible();
      await expect(page.getByLabel(/^Secret name/)).toHaveValue("counters-api");
      expect(await axeViolations(page)).toEqual([]);
      await page.screenshot({ path: `test-results/secret-form-empty-${width}.png`, fullPage: true });
      await page.getByLabel(/^Value/).fill("typed-in-the-form");
      await page.getByRole("button", { name: "Encrypt and propose" }).click();
      await expect(page.getByText("pr-7")).toBeVisible();
      await expect(page.getByLabel(/^Value/)).toHaveValue("");
      await page.screenshot({ path: `test-results/secret-form-proposed-${width}.png`, fullPage: true });
    });

    test("a refused field is named", async ({ page }) => {
      await stubApi(page, true);
      await page.goto(`/projects/${PROJECT}/secrets/new?name=counters-api&key=apiKey&lang=en`);
      await page.getByLabel(/^Value/).fill("x");
      await page.getByRole("button", { name: "Encrypt and propose" }).click();
      await expect(page.getByRole("alert")).toContainText("values.apiKey");
      await page.screenshot({ path: `test-results/secret-form-refused-${width}.png`, fullPage: true });
    });
  });
}
