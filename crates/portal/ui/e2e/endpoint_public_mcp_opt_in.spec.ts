import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-6761 (EP-24, deny by default): a public Endpoint serves MCP only when it opts in. The edit
// form shows MCP as a switch the person checks, with a hint; an internal Endpoint keeps it
// locked on, and turning one public turns MCP off until checked.

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const endpoint = (name: string, audience: string) => ({
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki", labels: { "joinedcontext.com/space": "ilma" } },
  spec: {
    contextSpaceRef: "ilma",
    slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
    audience,
    enabledRepresentations: ["ngsi-ld", "csv"],
    policyRefs: ["urn:ngsi-ld:Policy:hel.fi:helsinki-ilma:air-read"],
  },
  status: { phase: "Live" },
});
const ENDPOINTS = [endpoint("air-open", "public"), endpoint("air-team", "organization")];
const POLICY = {
  apiVersion: API,
  kind: "Policy",
  metadata: { name: "air-read", namespace: "helsinki" },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "ilma" },
    assigner: "did:web:{orgDomain}",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
};

async function stub(page: Page): Promise<void> {
  await page.route("**/api/v1/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    if (path.includes("/drafts/")) return route.fulfill({ status: 404, contentType: "application/json", body: "{}" });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "helsinki", grants: [{ role: "steward", rule: { verbs: ["read", "propose", "approve"], kinds: ["Endpoint", "Policy", "ContextSpace"] }, binding: "stewards", scope: "project:helsinki" }] });
    }
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path.endsWith("/organizations")) return json(list([{ apiVersion: API, kind: "Organization", metadata: { name: "hel", namespace: "helsinki" }, spec: { domain: "hel.fi" } }]));
    if (path === "/api/v1/projects/helsinki/spaces") {
      return json(list([{ apiVersion: API, kind: "ContextSpace", metadata: { name: "ilma", namespace: "helsinki" }, spec: {}, status: { phase: "Live" } }]));
    }
    if (path === "/api/v1/projects/helsinki/policies") return json(list([POLICY]));
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
  test(`a public Endpoint's MCP is a switch, off until checked, at ${width} px (T-6761)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page);
    const dir = join("test-results", "T-6761");
    mkdirSync(dir, { recursive: true });

    // Internal: MCP is served and locked on, as before.
    await page.goto("/projects/helsinki/endpoints/air-team/edit?lang=en");
    const mcp = page.getByRole("checkbox", { name: "MCP for AI assistants", exact: true });
    await expect(mcp).toBeChecked();
    await expect(mcp).toBeDisabled();
    const line = page.getByTestId("public-mcp-hint");
    const hint = line.filter({ hasText: "stays off on a public endpoint until you check" });
    const on = line.filter({ hasText: "MCP is on: AI assistants" });
    await expect(line).toHaveCount(0);

    // Turning it public turns MCP off until the person checks it, and the hint appears.
    await page.getByRole("combobox", { name: "Audience", exact: true }).selectOption({ label: "Public: anyone with the address" });
    await expect(mcp).not.toBeChecked();
    await expect(mcp).toBeEnabled();
    await expect(hint).toBeVisible();

    // Public, not opted in: unchecked, choosable, and the hint says how to turn it on.
    await page.goto("/projects/helsinki/endpoints/air-open/edit?lang=en");
    await expect(mcp).not.toBeChecked();
    await expect(mcp).toBeEnabled();
    await expect(hint).toContainText("Example:");
    await expect(on).toHaveCount(0);
    await hint.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `public-mcp-off-${width}.png`) });

    // Checked, the line beside it says what is on and how to turn it off.
    await mcp.check();
    await expect(mcp).toBeChecked();
    await expect(on).toBeVisible();
    await expect(hint).toHaveCount(0);
    await mcp.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `public-mcp-on-${width}.png`) });
  });
}
