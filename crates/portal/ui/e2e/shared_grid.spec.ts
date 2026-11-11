import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6739: Policies, Approvals and Registrations on the shared data grid (the SDK's DataGrid),
// walked in the built bundle with the API answered from the browser. Each state is shot at
// 1440 and 400 px into test-results/T-6739 for the UI judge.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-admin"] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] };

const change = (name: string, endpoint: string, day: number, lane: string, phase: string, author: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name, namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: endpoint, fields: 2 } },
  author: { name: author, email: `${author.split(" ")[0]!.toLowerCase()}@hel.fi` },
  createdAt: `2026-11-${String(day).padStart(2, "0")}T09:30:00Z`,
  status: { lane, phase, plan: { update: 1 } },
});
const CHANGES = [
  change("chg-1a2b3c4d", "air-quality", 9, "yellow", "PendingApproval", "Marek Mráz"),
  change("chg-5e6f7a8b", "bus-stops", 10, "green", "Deploying", "Aino Virtanen"),
  change("chg-9c0d1e2f", "cycling-counters", 4, "red", "PendingApproval", "Jana Kováčová"),
];
const policy = (name: string, title: string, space: string, assignee: { kind: string; id: string }, operations: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { contextSpaceRef: space, assigner: "did:web:hel.fi", assignee, operations },
  status: { phase: "Live" },
});
const POLICIES = [
  policy("air-readers", "Air quality readers", "air", { kind: "role", id: "viewer" }, ["retrieveOps"]),
  policy("transport-editors", "Transport editors", "transport", { kind: "group", id: "transport-team" }, ["retrieveOps", "updateOps"]),
  policy("open-events", "Open events", "events", { kind: "role", id: "anonymous" }, ["retrieveOps"]),
];
const registration = (name: string, space: string, endpoint: string, type: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSourceRegistration",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space, endpointRef: endpoint, information: [{ entities: [{ type }] }] },
  status: { phase: "Live" },
});
const CSRS = [registration("espoo-air", "air", "espoo-air-quality", "AirQualityObserved"), registration("vantaa-parking", "transport", "vantaa-parking", "ParkingSpot")];

type Mode = "filled" | "empty" | "error";

async function stub(page: Page, mode: Mode): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) =>
      mode === "error"
        ? route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ type: "about:blank", title: "Unavailable", status: 503, detail: "The configuration store did not answer. Try again in a minute." }) })
        : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: mode === "empty" ? [] : items });
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json(PROJECTS);
    if (url.pathname.endsWith("/helsinki/changes")) return list(CHANGES);
    if (url.pathname.endsWith("/helsinki/policies")) return list(POLICIES);
    if (url.pathname.endsWith("/helsinki/csrs")) return list(CSRS);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const PAGES = [
  { slug: "approvals", path: "/projects/helsinki/approvals", first: /air-quality/ },
  { slug: "policies", path: "/projects/helsinki/policies", first: /Air quality readers/ },
  { slug: "registrations", path: "/projects/helsinki/csrs", first: /espoo-air/ },
];

for (const width of [1440, 400]) {
  for (const target of PAGES) {
    test(`${target.slug} on the shared grid at ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      const shot = (state: string) => page.screenshot({ path: `test-results/T-6739/${target.slug}-${state}-${width}.png`, fullPage: true });

      await stub(page, "filled");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.getByRole("link", { name: target.first }).first()).toBeVisible();
      await expect(page.locator(".jc-data-grid")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot("filled");

      if (width === 1440) {
        await page.locator(".jc-data-grid thead .jc-grid-meta-btn").nth(1).click();
        await expect(page.getByRole("menu")).toBeVisible();
        await shot("menu");
        await page.keyboard.press("Escape");
      }
      await page.getByRole("button", { name: "Arrange columns" }).click();
      await expect(page.getByRole("dialog", { name: "Arrange columns" })).toBeVisible();
      await shot("arrange");
      await page.getByRole("button", { name: "Done" }).click();

      const search = page.getByRole("searchbox", { name: target.slug === "policies" ? "Find a policy" : "Search this list" });
      await search.fill("zzz-nothing");
      await shot("nomatch");

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, "empty");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.locator(".jc-data-grid")).toBeVisible();
      await shot("empty");

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, "error");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.getByText(/configuration store did not answer/)).toBeVisible();
      await shot("error");
    });
  }
}
