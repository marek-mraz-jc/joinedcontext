import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-5531: who may do what on one Endpoint, every principal at once (API/01 §57). Each state at
// 1440 and 400 px: the matrix, a partial cell's detail, the graph, the refusal, no Policy yet and
// a gateway that does not answer.

const API = "joinedcontext.com/v1alpha1";
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "helsinki-bikes-ops", namespace: "helsinki" },
  spec: { contextSpaceRef: "bikes", slug: "helsinki-bikes-ops", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
};
const TYPES = ["BikeHireDockingStation", "Vehicle"];
const PRINCIPALS = [
  { kind: "group", name: "bike-stewards", policy: "stewards-write", write: ["status"], q: 'operator=="HSL"' },
  { kind: "group", name: "bike-mechanics", policy: "mechanics-write", write: ["status", "freeSlots"] },
  { kind: "group", name: "city-analysts", policy: "analysts-history" },
  { kind: "role", name: "endpoint:helsinki/helsinki-bikes-ops", policy: "app-readers" },
  { kind: "role", name: "data-steward", policy: "steward-all", all: true },
  { kind: "person", name: "jana@hel.fi", policy: "jana-read" },
  { kind: "person", name: "mikko@hel.fi", policy: "mikko-read" },
  { kind: "serviceAccount", name: "helsinki-bikes-feed", policy: "feed-write", write: ["*"] },
  { kind: "serviceAccount", name: "helsinki-ckan", policy: "ckan-read" },
  { kind: "public", name: "public", policy: "public-read", prohibit: ["owner"] },
];

function matrix() {
  const cells: unknown[] = [];
  const edges: { from: string; to: string }[] = [];
  for (const p of PRINCIPALS) {
    const id = `${p.kind}:${p.name}`;
    edges.push({ from: `principal:${id}`, to: `policy:${p.policy}` }, { from: `policy:${p.policy}`, to: "endpoint:helsinki-bikes-ops" });
    for (const type of TYPES) {
      const cell = (action: string, attributes: unknown, extra: Record<string, unknown> = {}) =>
        cells.push({ principal: id, type, action, effect: "permit", attributes, constraint: null, decidedBy: p.policy, ...extra });
      cell("read", "*");
      if (p.name === "city-analysts" || p.all) cell("history", "*");
      if (p.write && type === TYPES[0]) cell("write", p.write[0] === "*" ? "*" : p.write, p.q ? { constraint: `q: ${p.q}` } : {});
      if (p.all) {
        cell("write", "*");
        cell("delete", "*");
      }
      if (p.prohibit && type === TYPES[0]) {
        cells.push({ principal: id, type, action: "read", effect: "prohibit", attributes: p.prohibit, constraint: null, decidedBy: "no-owner" });
      }
    }
  }
  for (const type of TYPES) edges.push({ from: "endpoint:helsinki-bikes-ops", to: `type:${type}` });
  return {
    endpoint: "helsinki-bikes-ops",
    space: "bikes",
    principals: PRINCIPALS.map((p) => ({ kind: p.kind, name: p.name, via: [{ kind: "policy", name: p.policy }] })),
    types: TYPES,
    cells,
    edges,
  };
}

type State = "matrix" | "detail" | "graph" | "refused" | "empty" | "failed";

const problem = (status: number, title: string, detail: string) => ({
  status,
  contentType: "application/problem+json",
  body: JSON.stringify({ type: "about:blank", title, status, detail }),
});

async function stub(page: Page, state: State) {
  const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
  await page.route("**/api/v1/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "anna", name: "Anna", email: "anna@hel.fi", roles: [] });
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path.endsWith("/access/matrix")) {
      if (state === "refused") {
        return route.fulfill(
          problem(403, "Forbidden", "the access matrix is for organization administrators and for whoever reads Policies in the Endpoint's space"),
        );
      }
      if (state === "failed") return route.fulfill(problem(503, "Service Unavailable", "the gateway did not answer; try again shortly"));
      if (state === "empty") return json({ ...matrix(), principals: [], types: [], cells: [], edges: [] });
      return json(matrix());
    }
    if (path === "/api/v1/projects/helsinki/endpoints/helsinki-bikes-ops") return json(ENDPOINT);
    return json(list([]));
  });
  await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));
}

for (const width of [1440, 400]) {
  for (const state of ["matrix", "detail", "graph", "refused", "empty", "failed"] as const) {
    test(`who may do what on an Endpoint at ${width} px: ${state} (T-5531)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, state);
      const dir = join("test-results", "T-5531");
      mkdirSync(dir, { recursive: true });

      await page.goto("/projects/helsinki/endpoints/helsinki-bikes-ops?lang=en");
      const section = page.locator("section", { has: page.getByRole("heading", { name: "Who may do what", exact: true }) });
      await expect(section.getByLabel("Show")).toBeVisible();
      if (state === "matrix" || state === "detail") {
        await expect(section.getByRole("button", { name: "The public, Read BikeHireDockingStation: In part" })).toBeVisible();
        if (state === "detail") {
          await section.getByRole("button", { name: "bike-stewards, Write BikeHireDockingStation: In part" }).click();
          await expect(section.getByText("Only these attributes: status")).toBeVisible();
          await expect(section.getByRole("link", { name: "stewards-write" })).toBeVisible();
        }
      } else if (state === "graph") {
        await section.getByRole("tab", { name: "Graph" }).click();
        await expect(section.getByRole("group", { name: "How each principal reaches helsinki-bikes-ops" })).toBeVisible();
      } else if (state === "refused") {
        await expect(section.getByText(/whoever reads Policies/)).toBeVisible();
      } else if (state === "empty") {
        await expect(section.getByText("No Policy names anyone on this Endpoint yet")).toBeVisible();
      } else {
        await expect(section.getByText(/the gateway did not answer/)).toBeVisible();
      }
      await section.scrollIntoViewIfNeeded();
      await section.screenshot({ path: join(dir, `${state}-${width}.png`) });
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }
}
