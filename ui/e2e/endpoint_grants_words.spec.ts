import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// T-5592: helsinki-bikes-ops' page as a viewer sees it. "Who may call it" names who may write
// `status` in words, the greyed header actions say why, and the page is no wider than a 400 px phone
// (it was 409 px on dev).

const API = "joinedcontext.com/v1alpha1";
const SUBSCRIPTIONS = ["createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"];
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "helsinki-bikes-ops", namespace: "helsinki", title: { en: "City bikes, operations" } },
  spec: { contextSpaceRef: "helsinki", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "organization", enabledRepresentations: ["ngsi-ld"],
    // The code-list IRI dev's catalogue block carries: one unbreakable word that made the page 409 px.
    catalog: { spatial: ["FI1B1", "http://uri.suomi.fi/codelist/jhs/kunta_1_20250101/code/091"] } },
  status: { phase: "Live" },
};
const SPACE = { apiVersion: API, kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: {} };
const policy = (name: string, title: string, spec: Record<string, unknown>) => ({
  apiVersion: API,
  kind: "Policy",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { contextSpaceRef: { kind: "ContextSpace", name: "helsinki" }, ...spec },
});
const STATIONS = [{ entities: [{ type: "BikeHireDockingStation" }] }];
const POLICIES = [
  policy("bikes-ops-read", "The city's people read the bike stations", {
    assignee: { kind: "group", id: "helsinki" },
    operations: ["retrieveOps", ...SUBSCRIPTIONS],
    information: STATIONS,
  }),
  policy("bikes-ops-steward", "The steward sets a station's status", {
    assignee: { kind: "user", id: "demo.steward@hel.fi" },
    operations: ["retrieveOps", "updateAttrs", ...SUBSCRIPTIONS],
    information: [{ ...STATIONS[0], propertyNames: ["status"] }],
  }),
  policy("pipelines-write", "The pipelines write the stations", {
    assignee: { kind: "serviceAccount", id: "pipelines" },
    operations: ["createBatch", "upsertBatch", "updateBatch"],
    information: STATIONS,
  }),
];
const PIPELINES = { apiVersion: API, kind: "ServiceAccount", metadata: { name: "pipelines", namespace: "helsinki", title: { en: "Helsinki pipelines" } }, spec: {} };

for (const width of [1440, 400]) {
  test(`a viewer reads who may write status on helsinki-bikes-ops at ${width} px (T-5592)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json({ subject: "v1", username: "viewer", name: "Vera Viewer", email: "v@hel.fi", roles: [] });
      if (path.endsWith("/permissions/me")) {
        return json({ project: "helsinki", grants: [{ role: "viewer", binding: "viewers", scope: "project", rule: { kinds: ["*", "Group", "ServiceAccount"], verbs: ["read"] } }] });
      }
      if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
      if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
      if (path === "/api/v1/projects/helsinki/endpoints/helsinki-bikes-ops") return json(ENDPOINT);
      if (path.endsWith("/spaces")) return json(list([SPACE]));
      if (path.endsWith("/policies")) return json(list(POLICIES));
      if (path.endsWith("/helsinki/serviceaccounts")) return json(list([PIPELINES]));
      return json(list([]));
    });
    await page.route("**/cs/**", (route) => route.fulfill({ contentType: "application/ld+json", body: "[]" }));
    await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));

    await page.goto("/projects/helsinki/endpoints/helsinki-bikes-ops?lang=en");
    const grants = page.locator("section", { has: page.getByRole("heading", { name: "Who may call it" }) });
    const steward = grants.getByRole("listitem").filter({ hasText: "The steward sets a station's status" });
    await expect(steward).toContainText("demo.steward@hel.fi");
    await expect(steward).toContainText("BikeHireDockingStation: only status");
    await expect(steward).toContainText("changes data");
    await expect(grants.getByRole("listitem").filter({ hasText: "read the bike stations" })).toContainText("manages subscriptions");
    await expect(grants).toContainText("Helsinki pipelines");
    await expect(grants).not.toContainText("serviceAccount:");

    // The greyed actions say why, in a line one reads at any width.
    await expect(page.getByRole("button", { name: "Change these settings" })).toHaveAttribute("aria-disabled", "true");
    await expect(page.getByText("Disabled: your role does not permit 'propose' on 'Endpoint' in this project").first()).toBeVisible();

    const wide = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(wide).toBeLessThanOrEqual(width);

    const dir = join("test-results", "T-5592");
    mkdirSync(dir, { recursive: true });
    await page.screenshot({ path: join(dir, `viewer-endpoint-${width}.png`), fullPage: true });
    await grants.scrollIntoViewIfNeeded();
    await grants.screenshot({ path: join(dir, `who-may-call-${width}.png`) });
  });
}
