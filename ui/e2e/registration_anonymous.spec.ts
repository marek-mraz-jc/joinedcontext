import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-6545 (PF-48): a registration of a public address elsewhere can forward as nobody, so no token
// or account leaves the platform. The choice is offered only for an address, and an anonymous
// source reached over plain http is refused at the field. `vite preview` has no Portal API behind
// it, so the API is answered in the browser.

const PROJECT = "banskabystrica";
const EVIDENCE = process.env.JC_EVIDENCE_DIR;
const NOBODY = "Nobody: a public address elsewhere";

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "a1", username: "jana", name: "Jana Kováčová", email: "jana@example.test", roles: ["portal-admin"] });
    }
    if (path.endsWith("/branding")) return json({ documentationBaseUrl: "https://docs.example.test" });
    if (path.endsWith("/permissions/me")) {
      return json({
        project: PROJECT,
        bootstrap: false,
        grants: [
          {
            binding: "b",
            role: "admin",
            scope: `project:${PROJECT}`,
            rule: { kinds: ["ContextSourceRegistration", "ContextSpace", "Endpoint", "ServiceAccount"], verbs: ["read", "propose", "approve", "delete"] },
          },
        ],
      });
    }
    if (path.includes("/drafts/")) {
      const manifest = (route.request().postDataJSON() as { manifest?: unknown } | null)?.manifest;
      if (manifest === undefined) return route.fulfill({ status: 404, contentType: "application/json", body: "{}" });
      return json({ manifest, version: 1, touchedBy: "jana", touchedKind: "person", updatedAt: "2026-11-03T07:00:00Z" });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

async function shot(page: Page, name: string): Promise<void> {
  if (!EVIDENCE) return;
  mkdirSync(EVIDENCE, { recursive: true });
  await page.screenshot({ path: join(EVIDENCE, `${name}.png`) });
}

for (const width of [1440, 400]) {
  test.describe(`an anonymous registration at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("is offered for an address elsewhere and needs https", async ({ page }) => {
      await stubApi(page);
      await page.goto(`/projects/${PROJECT}/csrs?lang=en`);
      await page.getByRole("main").getByRole("button", { name: "New registration" }).first().click();
      const dialog = page.getByTestId("form-page");
      const identity = dialog.getByRole("combobox", { name: "Forward as" });
      await expect(identity).toBeVisible({ timeout: 20_000 });
      await expect(identity.locator("option", { hasText: NOBODY })).toHaveCount(0);

      await dialog.getByRole("radio", { name: "An NGSI-LD API elsewhere" }).check();
      await identity.selectOption({ label: NOBODY });
      await dialog.locator("#root_endpoint").fill("http://api.example.org/api/endpoint/x");
      await dialog.locator("#root_name").fill("prod-parcels");
      await dialog.locator("#root_endpoint").blur();
      await identity.scrollIntoViewIfNeeded();
      await expect(dialog.getByText("A full address starting with https://.")).toBeVisible();
      await shot(page, `anonymous-http-${width}`);

      await dialog.locator("#root_endpoint").fill("https://api.smartcity.joinedcontext.com/api/endpoint/c223begxsfspzmdaq5ij4aayw6vyucjl");
      await identity.scrollIntoViewIfNeeded();
      await expect(dialog.getByText("A full address starting with https://.")).toHaveCount(0);
      await shot(page, `anonymous-https-${width}`);
      await expect(identity.locator("option:checked")).toHaveText(NOBODY);
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
