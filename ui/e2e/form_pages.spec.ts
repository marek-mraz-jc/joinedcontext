import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-2474, UI-27: a bigger form is a page with an address of its own. `vite preview` has no
// Portal API behind it, so the API is answered in the browser; the router, the reload and the
// browser's history are the real ones.

const PROJECT = "banskabystrica";
const LIST = `/projects/${PROJECT}/policies`;

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "open-read", namespace: PROJECT },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
    assigner: "did:web:banskabystrica.sk",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps"],
  },
};

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "ovzdusie", namespace: PROJECT },
  spec: {},
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) => json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith("/policies/open-read")) return json(POLICY);
    if (path.endsWith("/policies")) return list([POLICY]);
    if (path.endsWith("/spaces")) return list([SPACE]);
    return list([]);
  });
}

test.describe("forms at their own addresses", () => {
  test("the create form is a page at /new, a reload keeps it, and its back control returns to the list", async ({
    page,
  }) => {
    await stubApi(page);

    await page.goto(`/projects/${PROJECT}/policies/new?lang=en`);
    const form = page.getByRole("region", { name: "New Policy" });
    await expect(form).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // The sidebar stays: the form is a page of the Portal, not an overlay over it.
    await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();

    await page.reload();
    await expect(form).toBeVisible();

    // The back button is gone from the form; the breadcrumb's list link returns to the list.
    await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Policies" }).click();
    await expect(page).toHaveURL(new RegExp(`${LIST}$`));
    await expect(page.getByRole("heading", { level: 1, name: "Policies" })).toBeVisible();
  });

  test("a row's edit opens at its address and the browser's back button returns to the list", async ({
    page,
  }) => {
    await stubApi(page);

    await page.goto(`${LIST}?lang=en`);
    await page.getByRole("button", { name: "More actions for open-read" }).click();
    await page.getByRole("menuitem", { name: /Edit/ }).click();
    await expect(page).toHaveURL(new RegExp(`${LIST}/open-read/edit`));
    await expect(page.getByRole("region", { name: "Edit open-read" })).toBeVisible();

    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`${LIST}(\\?|$)`));
    await expect(page.getByRole("region", { name: "Edit open-read" })).toHaveCount(0);
    await expect(page.getByRole("heading", { level: 1, name: "Policies" })).toBeVisible();
  });

  test("a deep-linked edit form has no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto(`/projects/${PROJECT}/policies/open-read/edit?lang=sk`);
    await expect(page.getByTestId("form-page")).toBeVisible();
    await expect(page.getByTestId("form-page").getByRole("textbox").first()).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
