import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-5052: the YAML view of a manifest in a real browser, the API answered in the browser. A
// viewer reads it in Monaco read-only; a steward edits it, sees what a field is for on hover, and
// a parse error and a refusal naming a field are marked at their line. Shots at 1440 and 400 px.

const API = "joinedcontext.com/v1alpha1";
const dir = join("test-results", "T-5052");
const LIST = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });

const BINDING = {
  apiVersion: API,
  kind: "RoleBinding",
  metadata: { name: "stewards", namespace: "org", title: { en: "Helsinki stewards" } },
  spec: { subjects: [{ user: "eva@hel.fi" }], role: "steward", scope: { project: "helsinki" } },
  status: { phase: "Live" },
};

const SCHEMA = {
  type: "object",
  properties: {
    spec: {
      type: "object",
      properties: {
        role: { type: "string", description: "The Role this binding grants, by name, e.g. steward." },
        subjects: { type: "array", description: "Who holds the role: people, groups or service accounts." },
      },
    },
  },
};

function grants(propose: boolean) {
  return {
    project: "org",
    bootstrap: false,
    grants: [
      {
        role: propose ? "org-admin" : "viewer",
        binding: "b",
        rule: { kinds: ["RoleBinding", "Role", "Project"], verbs: propose ? ["read", "propose"] : ["read"] },
      },
    ],
  };
}

async function stub(page: Page, propose: boolean, refuse = false) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: status >= 400 ? "application/problem+json" : "application/json",
        body: JSON.stringify(body),
      });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "eva", name: "Eva", email: "eva@hel.fi", roles: ["portal-editor"] });
    }
    if (path.endsWith("/permissions/me")) return json(grants(propose));
    if (path === "/api/v1/kinds/RoleBinding/schema") return json(SCHEMA);
    if (path === "/api/v1/projects") return json(LIST([{ name: "helsinki" }]));
    if (path === "/api/v1/projects/org/rolebindings/stewards" && request.method() === "GET") return json(BINDING);
    if (path === "/api/v1/projects/org/rolebindings") return json(LIST([BINDING]));
    if (refuse && request.method() !== "GET") {
      return json(
        { type: "about:blank", title: "Unprocessable", status: 422, detail: "spec.role: no Role named stewart", errors: ["spec.role: no Role named stewart"] },
        422,
      );
    }
    return json(LIST([]));
  });
}

const editor = (page: Page) => page.locator(".monaco-editor").first();

for (const width of [1440, 400]) {
  test.describe(`the YAML view at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      mkdirSync(dir, { recursive: true });
    });

    test("a viewer reads the manifest read-only", async ({ page }) => {
      await stub(page, false);
      await page.goto("/projects/helsinki/settings/members?edit=stewards&lang=en");
      // T-5591: a reader's heading is the record's own title, "Read only" beside it.
      await expect(page.getByRole("heading", { name: "eva@hel.fi: steward" })).toBeVisible();
      await expect(page.getByText("Read only", { exact: true })).toBeVisible();
      await expect(editor(page)).toBeVisible();
      await expect(editor(page)).toContainText("apiVersion");
      await expect(editor(page)).not.toContainText("status");
      // T-5513: Monaco carries only the YAML grammar now; keys and values still get their own
      // colours, and a mapping still folds.
      const key = editor(page).locator(".view-line span span", { hasText: "apiVersion" }).first();
      const value = editor(page).locator(".view-line span span", { hasText: "RoleBinding" }).first();
      await expect
        .poll(async () => (await key.getAttribute("class")) !== (await value.getAttribute("class")))
        .toBe(true);
      await expect(editor(page).locator(".codicon-folding-expanded").first()).toBeAttached();
      await editor(page).click();
      await page.keyboard.type("x");
      await expect(editor(page)).not.toContainText("xapiVersion");
      await page.screenshot({ path: join(dir, `viewer-${width}.png`) });
    });

    test("a steward sees a field's description on hover and a parse error at its line", async ({ page }) => {
      await stub(page, true);
      await page.goto("/projects/helsinki/settings/members?edit=stewards&lang=en");
      await expect(editor(page)).toContainText("role: steward");
      await editor(page).getByText("role", { exact: true }).first().hover();
      await expect(page.getByText("The Role this binding grants")).toBeVisible();
      await page.screenshot({ path: join(dir, `hover-${width}.png`) });
      await editor(page).getByText("role", { exact: true }).first().click();
      await page.keyboard.press("End");
      await page.keyboard.type(": [");
      await expect(page.locator(".squiggly-error").first()).toBeVisible();
      await expect(page.getByRole("main").getByRole("alert")).toContainText(/Line \d+: .+\. Fix the YAML to propose the change\.$/);
      await expect(page.getByRole("button", { name: "Propose change" })).toHaveAttribute("aria-disabled", "true");
      await page.screenshot({ path: join(dir, `parse-error-${width}.png`) });
    });

    test("a refusal naming a field is marked at its line", async ({ page }) => {
      await stub(page, true, true);
      await page.goto("/projects/helsinki/settings/members?edit=stewards&lang=en");
      await expect(editor(page)).toContainText("role: steward");
      await page.getByRole("button", { name: "Propose change" }).click();
      await expect(page.getByRole("main").getByRole("alert")).toContainText("no Role named stewart");
      await expect(page.locator(".squiggly-error").first()).toBeVisible();
      await page.screenshot({ path: join(dir, `refusal-${width}.png`) });
    });
  });
}
