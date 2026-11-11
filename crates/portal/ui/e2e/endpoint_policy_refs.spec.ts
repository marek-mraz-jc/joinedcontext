import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-6740 (P-18): an Endpoint names the Policies it serves. The editor offers the space's Policies
// the person reads as checkboxes with a hint, says what to do when there is none, and a Policy's
// check lists the Endpoints whose calls it decides.

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const endpoint = (name: string, space: string) => ({
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki", labels: { "joinedcontext.com/space": space } },
  spec: { contextSpaceRef: space, slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "organization", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
});
const policy = (name: string) => ({
  apiVersion: API,
  kind: "Policy",
  metadata: { name, namespace: "helsinki" },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "ilma" },
    assigner: "did:web:{orgDomain}",
    assignee: { kind: "group", id: "helsinki" },
    operations: ["retrieveOps"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
});
const SPACES = [
  { apiVersion: API, kind: "ContextSpace", metadata: { name: "ilma", namespace: "helsinki" }, spec: {}, status: { phase: "Live" } },
  { apiVersion: API, kind: "ContextSpace", metadata: { name: "tyhja", namespace: "helsinki" }, spec: {}, status: { phase: "Live" } },
];
const ENDPOINTS = [endpoint("air-open", "ilma"), endpoint("empty-door", "tyhja")];
const POLICIES = [policy("air-read"), policy("air-partners")];

async function stub(page: Page): Promise<void> {
  await page.route("**/api/v1/**", (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "helsinki", grants: [{ role: "steward", rule: { verbs: ["read", "propose", "approve"], kinds: ["Endpoint", "Policy", "ContextSpace"] }, binding: "stewards", scope: "project:helsinki" }] });
    }
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path.endsWith("/organizations")) return json(list([{ apiVersion: API, kind: "Organization", metadata: { name: "hel", namespace: "helsinki" }, spec: { domain: "hel.fi" } }]));
    if (path === "/api/v1/projects/helsinki/spaces") return json(list(SPACES));
    if (path === "/api/v1/projects/helsinki/policies" && request.method() === "POST") {
      // The check of a Policy: valid, and the Endpoints it binds (CHG-11 lite).
      return json({ valid: true, lane: "red", plan: { fields: [] }, bindsEndpoints: ["air-open", "air-partners-api"] });
    }
    if (path === "/api/v1/projects/helsinki/policies") return json(list(POLICIES));
    if (path === "/api/v1/projects/helsinki/policies/air-read") return json(POLICIES[0]);
    for (const one of ENDPOINTS) {
      if (path === `/api/v1/projects/helsinki/endpoints/${one.metadata.name}`) return json(one);
    }
    if (path === "/api/v1/projects/helsinki/endpoints") return json(list(ENDPOINTS));
    return json(list([]));
  });
  await page.route("**/cs/**", (route) => route.fulfill({ contentType: "application/ld+json", body: "[]" }));
  await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));
}

for (const width of [1440, 400]) {
  test(`an Endpoint names the Policies it serves, and a Policy's check names its Endpoints at ${width} px (T-6740)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page);
    const dir = join("test-results", "T-6740");
    mkdirSync(dir, { recursive: true });

    // Naming none, it binds every Policy of its space today: the edit opens with both ticked.
    await page.goto("/projects/helsinki/endpoints/air-open/edit?lang=en");
    const read = page.getByRole("checkbox", { name: "air-read", exact: true });
    await expect(read).toBeChecked();
    await expect(page.getByRole("checkbox", { name: "air-partners", exact: true })).toBeChecked();
    await expect(page.getByText("Tick the Policies whose grants decide this Endpoint's calls")).toBeVisible();
    await read.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `endpoint-policies-filled-${width}.png`) });

    // A space without a Policy the person reads says where one comes from.
    await page.goto("/projects/helsinki/endpoints/empty-door/edit?lang=en");
    const none = page.getByTestId("no-policies");
    await expect(none).toContainText("holds no Policy you may read");
    await expect(none.getByRole("link", { name: "Open the policies" })).toBeVisible();
    await none.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `endpoint-policies-empty-${width}.png`) });

    // A Policy's check lists the Endpoints whose calls it decides.
    await page.goto("/projects/helsinki/policies/new?lang=en");
    await page.locator("#root_name").fill("air-research");
    await page.getByRole("combobox", { name: "Context Space" }).selectOption("ilma");
    await page.getByRole("textbox", { name: "Grantee" }).fill("researchers");
    await page.getByRole("button", { name: "Check", exact: true }).first().click();
    const binds = page.getByTestId("binds-endpoints");
    await expect(binds).toContainText("air-partners-api");
    await binds.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `policy-binds-endpoints-${width}.png`) });
  });
}
