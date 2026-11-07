import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// The secret form (T-5971, T-6048): a viewer the secrets door refuses keeps the page's one h1 and
// its purpose line above the refusal, the walker's one-h1 rule; a steward gets the form under the
// same heading. `vite preview` has no Portal API behind it, so the API is answered in the browser.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

const IDENTITY = { subject: "b7c1e0f4", username: "ana", name: "Ana Admin", email: "ana@hel.fi", roles: [] };

async function stubApi(page: Page, refused: boolean): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      const verbs = refused ? ["read"] : ["read", "propose"];
      return json({ project: "helsinki", bootstrap: false, grants: [
        { role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["*"], verbs } },
      ] });
    }
    if (path === "/api/v1/projects/helsinki/secrets") {
      return refused
        ? json({ type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: "you may not read the secrets of helsinki" }, 403)
        : json({ items: [] });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, label] of [[1440, "desktop"], [400, "phone"]] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: join(dir, `secret-form-${name}-${label}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function oneHeading(page: Page): Promise<void> {
  await expect(page.locator("h1")).toHaveText(["Enter a secret"]);
  const purpose = await page.locator("h1").evaluate((h1) => h1.nextElementSibling?.tagName);
  expect(purpose).toBe("P");
}

test.describe("the secret form", () => {
  test("a refused viewer keeps the page's heading above the refusal", async ({ page }) => {
    await stubApi(page, true);
    await page.goto("/projects/helsinki/secrets/new?name=counters-api&key=apiKey&lang=en");
    await expect(page.getByText("you may not read the secrets of helsinki")).toBeVisible();
    await oneHeading(page);
    await expect(page.getByLabel(/^Value/)).toHaveCount(0);
    await shot(page, "refused");
  });

  test("a steward gets the form under the same heading", async ({ page }) => {
    await stubApi(page, false);
    await page.goto("/projects/helsinki/secrets/new?name=counters-api&key=apiKey&lang=en");
    await expect(page.getByRole("button", { name: "Encrypt and propose" })).toBeVisible();
    await oneHeading(page);
    await shot(page, "form");
  });

  test("a key typed twice or left blank is named at its field and nothing is sent (F-R2UI4-017)", async ({ page }) => {
    await stubApi(page, false);
    const posted: string[] = [];
    page.on("request", (request) => {
      if (request.method() === "POST" && request.url().endsWith("/secrets")) posted.push(request.url());
    });
    await page.goto("/projects/helsinki/secrets/new?name=counters-api&key=apiKey&lang=en");
    await page.getByRole("button", { name: "Add a key" }).click();
    await page.getByRole("button", { name: "Add a key" }).click();
    await page.getByRole("textbox", { name: /^Key/ }).nth(1).fill("apiKey");
    await page.getByRole("button", { name: "Encrypt and propose" }).click();
    await expect(page.getByText("The key apiKey is already above: give each value its own key.")).toBeVisible();
    await expect(page.getByText("Name this value, e.g. apiKey.")).toBeVisible();
    expect(posted).toEqual([]);
    await shot(page, "key-twice");
  });
});
