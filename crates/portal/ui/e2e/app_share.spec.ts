import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// Sharing an App (T-4913; AP-173, AP-174): an owner opens Share on a card, reads whom each choice
// reaches, proposes the organization and sees it wait for approval; a person of another project
// finds the App under Shared with me. `vite preview` has no Portal API, so the API is answered in
// the browser; the pages are the real build.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const APP = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "App",
  metadata: { name: "air-map", namespace: "helsinki", title: { en: "Air quality map" } },
  status: { phase: "Live", build: { digest: "sha256:ab", commit: "9f1c2ab" } },
  spec: {
    kind: "static",
    visibility: "project",
    lifecycle: "published",
    dataNeeds: [],
    roles: [{ name: "inspector", title: { en: "Inspector" } }],
  },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

async function stub(page: Page, publicAllowed = true, shared: unknown[] = []): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "e1", username: "eva", name: "Eva Owner", email: "eva@hel.fi", roles: [] });
    }
    if (path.endsWith("/branding")) return json({ organisation: "Mestský úrad Banská Bystrica", appsOrigin: "https://dev.example.sk" });
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path.endsWith("/permissions/me")) {
      return json({ project: "helsinki", grants: [{ rule: { kinds: ["App", "Endpoint"], verbs: ["read", "propose"] } }] });
    }
    if (path === "/api/v1/shared-apps") return json(shared);
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path.endsWith("/audience")) {
      return json({ project: 7, organization: 42, roles: [{ name: "inspector", people: 3 }], publicAllowed });
    }
    if (path.endsWith("/apps") && request.method() === "GET") return json(list([APP]));
    if (path.endsWith("/build")) return json({ title: "Not Found", status: 404 }, 404);
    if (path.endsWith("/app-checks")) return json({ checks: [] });
    if (path.endsWith("/agent-runs")) return json({ items: [] });
    if (request.method() !== "GET") {
      return url.searchParams.get("dryRun") === "All" ? json({ valid: true, verdict: { ok: true, findings: [] } }) : json(CHANGE, 202);
    }
    return json(list([]));
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test(`an owner shares an App with the organization at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stub(page);
    await page.goto("/projects/helsinki/apps?lang=en");
    await page.getByRole("button", { name: /More actions for Air quality map/ }).click();
    await page.getByRole("menuitem", { name: /^Share/ }).click();
    const dialog = page.getByRole("dialog", { name: "Share Air quality map" });
    await expect(dialog.getByText("Everyone in Mestský úrad Banská Bystrica: 42 people")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `share-dialog-${width}`);
    await dialog.getByRole("radio", { name: "Everyone in the organization" }).check();
    await shot(page, `share-choice-organization-${width}`);
    await dialog.getByRole("radio", { name: "Anyone on the internet" }).check();
    await shot(page, `share-choice-public-${width}`);
    await dialog.getByRole("radio", { name: "Everyone in the organization" }).check();
    await dialog.getByRole("button", { name: "Propose the change" }).click();
    await expect(dialog.getByText(/The change waits for approval/)).toBeVisible();
    await shot(page, `share-pending-${width}`);
  });

  test(`public refused by the organization says why at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stub(page, false);
    await page.goto("/projects/helsinki/apps?lang=en");
    await page.getByRole("button", { name: /More actions for Air quality map/ }).click();
    await page.getByRole("menuitem", { name: /^Share/ }).click();
    const dialog = page.getByRole("dialog", { name: "Share Air quality map" });
    await expect(dialog.getByRole("radio", { name: "Anyone on the internet" })).toBeDisabled();
    await shot(page, `share-public-refused-${width}`);
  });

  test(`a person of another project finds it under Shared with me at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stub(page, true, [
      { project: "espoo", name: "air-map", title: { en: "Air quality map" }, description: { en: "Stations coloured by PM10" } },
      { project: "vantaa", name: "noise-desk", title: "Noise complaints", description: null },
    ]);
    await page.goto("/projects/helsinki/home?lang=en");
    const section = page.getByRole("region", { name: "Shared with me" });
    const link = section.getByRole("link", { name: /Air quality map/ });
    await expect(link).toHaveAttribute("href", "https://air-map.apps.dev.example.sk/?lang=en");
    expect(await axeViolations(page)).toEqual([]);
    await section.scrollIntoViewIfNeeded();
    await shot(page, `shared-with-me-${width}`);
  });

  test(`Shared with me says what happens next when empty at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stub(page);
    await page.goto("/projects/helsinki/home?lang=en");
    await expect(page.getByText(/Nothing is shared with you yet/)).toBeVisible();
    await shot(page, `shared-with-me-empty-${width}`);
  });
}
