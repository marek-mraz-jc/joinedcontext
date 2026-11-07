import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// PL-90..PL-93, T-4863: a pipeline asks for access to another space's Endpoint from its overview,
// and the owners of that space decide it on the Endpoint's page. `vite preview` has no Portal
// behind it: the API is answered in the browser, everything above it is the real page.

const SHOTS = process.env.JC_SHOTS_DIR;

const IDENTITY = { subject: "d3v", username: "dev", name: "Dana Developer", email: "dev@hel.fi", roles: [] };
const PERMISSIONS = { project: "helsinki", bootstrap: true, grants: [] };
const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "parking-sync", namespace: "helsinki" },
  spec: { class: "batch" },
  status: { phase: "Live" },
};
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "air-internal", namespace: "helsinki" },
  spec: { contextSpaceRef: "air", slug: "air-internal-k7m2qz4tv6xh3n5jb2ry", audience: "project-list" },
};
const request = (id: string, state: string, extra: Record<string, unknown> = {}) => ({
  id,
  pipeline: { project: "espoo", name: "parking-sync" },
  account: "espoo/pl-parking-sync",
  endpoint: { project: "helsinki", name: "air-internal" },
  space: "air",
  operations: "read",
  types: [{ type: "AirQualityObserved", attrs: ["pm10", "pm25", "dateObserved"] }],
  until: "2026-12-31T00:00:00Z",
  reason: "Parking guidance warns drivers on days with poor air.",
  state,
  requestedBy: "dev@espoo.fi",
  requestedAt: "2026-10-20T08:00:00Z",
  decidedBy: null,
  decidedAt: null,
  decisionReason: null,
  change: null,
  ...extra,
});
const FILLED = [
  request("3f6c1a2e-0b1d-4c55-9a7e-1d2c3b4a5f60", "pending"),
  request("7a8b9c0d-1e2f-4a3b-8c4d-5e6f7a8b9c0d", "approved", {
    decidedBy: "owner@hel.fi",
    decidedAt: "2026-10-20T09:30:00Z",
    change: "chg-5a1b2c3d",
  }),
  request("0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b", "declined", {
    operations: "write",
    decidedBy: "owner@hel.fi",
    decidedAt: "2026-10-19T15:00:00Z",
    decisionReason: "Writes to air go through the sensor pipeline only.",
  }),
];
const CONFLICT = {
  type: "about:blank",
  title: "Conflict",
  status: 409,
  detail: "This pipeline already has an open request for Endpoint air-internal of project helsinki: wait for its owners to decide it.",
};

const OFFERED = [
  { project: "helsinki", name: "air-internal", space: "air", types: [{ type: "AirQualityObserved", attrs: ["dateObserved", "no2", "pm10", "pm25"] }] },
  { project: "helsinki", name: "noise", space: "noise", types: [{ type: "NoiseLevelObserved", attrs: ["LAeq", "LAmax"] }] },
  { project: "vantaa", name: "traffic-counts", space: "roads", types: [] },
];

async function stubApi(page: Page, items: unknown[], refuseFile = false, offered: unknown[] = OFFERED): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json(PERMISSIONS);
    if (path.endsWith("/access-requests/endpoints")) return json({ items: offered });
    if (path.includes("/endpoints/") && path.endsWith("/access-requests")) return json({ types: [], refusal: null, items: [] });
    if (path.endsWith("/access-requests")) {
      if (req.method() === "POST") return refuseFile ? json(CONFLICT, 409) : json(items[0], 201);
      return json({ items });
    }
    if (path.endsWith("/pipelines/parking-sync")) return json(PIPELINE);
    if (path.endsWith("/endpoints/air-internal")) return json(ENDPOINT);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

/** Opens a picker, types part of a name and takes the option, as a person does. */
async function pick(box: ReturnType<Page["getByRole"]>, name: string): Promise<void> {
  await box.click();
  await box.fill(name.slice(0, 4));
  await box.page().getByRole("option", { name: new RegExp(name) }).first().click();
}

async function shot(page: Page, name: string, width: number): Promise<void> {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/access-requests-${name}-${width}.png`, fullPage: false });
}

for (const [width, height] of [[1440, 900], [400, 860]] as const) {
  test.describe(`pipeline access requests at ${width}`, () => {
    test.use({ viewport: { width, height } });

    test("an overview with no request offers to ask for one", async ({ page }) => {
      await stubApi(page, []);
      await page.goto("/projects/espoo/pipelines/parking-sync?lang=en#access");
      const section = page.getByTestId("pipeline-access");
      await expect(section.getByText("This pipeline has asked for no access to another space.")).toBeVisible();
      await section.scrollIntoViewIfNeeded();
      await shot(page, "empty", width);
    });

    test("the overview lists each request with its state", async ({ page }) => {
      await stubApi(page, FILLED);
      await page.goto("/projects/espoo/pipelines/parking-sync?lang=en#access");
      const section = page.getByTestId("pipeline-access");
      await expect(section.getByText("Waiting for the owner")).toBeVisible();
      await expect(section.getByText("Approved", { exact: true })).toBeVisible();
      await section.scrollIntoViewIfNeeded();
      await shot(page, "filled", width);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("an incomplete form says what is missing, a refusal says why", async ({ page }) => {
      await stubApi(page, [], true);
      await page.goto("/projects/espoo/pipelines/parking-sync?lang=en#access");
      const section = page.getByTestId("pipeline-access");
      await section.getByRole("button", { name: "Request access" }).click();
      await section.getByRole("button", { name: "Send request" }).click();
      await expect(section.getByRole("alert")).toBeVisible();
      await section.getByRole("alert").scrollIntoViewIfNeeded();
      await shot(page, "missing", width);
      await pick(section.getByRole("combobox", { name: "Endpoint" }), "air-internal");
      await pick(section.getByRole("combobox", { name: "Entity type" }), "AirQualityObserved");
      await pick(section.getByRole("combobox", { name: "Attributes" }), "pm10");
      await pick(section.getByRole("combobox", { name: "Attributes" }), "pm25");
      await section.getByLabel(/^Why the pipeline needs it/).fill("Parking guidance warns drivers on days with poor air.");
      await section.getByRole("button", { name: "Send request" }).click();
      await expect(section.getByText(/already has an open request/)).toBeVisible();
      await section.getByText(/already has an open request/).scrollIntoViewIfNeeded();
      await shot(page, "refusal", width);
    });

    test("the form offers the Endpoints, then their types and attributes", async ({ page }) => {
      await stubApi(page, []);
      await page.goto("/projects/espoo/pipelines/parking-sync?lang=en#access");
      const section = page.getByTestId("pipeline-access");
      await section.getByRole("button", { name: "Request access" }).click();
      const endpoint = section.getByRole("combobox", { name: "Endpoint" });
      await endpoint.scrollIntoViewIfNeeded();
      await shot(page, "picker-closed", width);
      await endpoint.click();
      await expect(page.getByRole("option", { name: /traffic-counts/ })).toBeVisible();
      await shot(page, "picker-open", width);
      await endpoint.fill("noi");
      await expect(page.getByRole("option", { name: /air-internal/ })).toHaveCount(0);
      await shot(page, "picker-searching", width);
      await page.getByRole("option", { name: /noise/ }).click();
      await section.getByRole("combobox", { name: "Entity type" }).click();
      await page.getByRole("option", { name: /NoiseLevelObserved/ }).click();
      await section.getByRole("combobox", { name: "Attributes" }).click();
      await expect(page.getByRole("option", { name: /LAeq/ })).toBeVisible();
      await shot(page, "picker-attributes", width);
      expect(await axeViolations(page)).toEqual([]);
      // An Endpoint whose space has no compiled model says so instead of an empty list.
      await pick(endpoint, "traffic-counts");
      await expect(section.getByText(/has no compiled data model/).first()).toBeVisible();
      await shot(page, "picker-no-types", width);
    });

    test("an Endpoint the list does not offer is asked for by its typed name", async ({ page }) => {
      await stubApi(page, []);
      await page.goto("/projects/espoo/pipelines/parking-sync?lang=en#access");
      const section = page.getByTestId("pipeline-access");
      await section.getByRole("button", { name: "Request access" }).click();
      const endpoint = section.getByRole("combobox", { name: "Endpoint" });
      await endpoint.click();
      await endpoint.fill("parks/trees");
      await expect(page.getByRole("option", { name: /Ask for parks\/trees/ })).toBeVisible();
      await shot(page, "picker-typed", width);
      await endpoint.press("Enter");
      await expect(section.getByText(/check this name against their data model/).first()).toBeVisible();
      const type = section.getByRole("combobox", { name: "Entity type" });
      await type.fill("TreeObserved");
      await type.press("Enter");
      const attrs = section.getByRole("combobox", { name: "Attributes" });
      await attrs.fill("height");
      await attrs.press("Enter");
      await attrs.scrollIntoViewIfNeeded();
      await shot(page, "picker-typed-filled", width);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("with no Endpoint to ask for, the form says how one comes to be", async ({ page }) => {
      await stubApi(page, [], false, []);
      await page.goto("/projects/espoo/pipelines/parking-sync?lang=en#access");
      const section = page.getByTestId("pipeline-access");
      await section.getByRole("button", { name: "Request access" }).click();
      await section.getByRole("combobox", { name: "Endpoint" }).click();
      await expect(page.getByText(/No Endpoint of another space is yours to read/)).toBeVisible();
      await shot(page, "picker-empty", width);
    });

    test("the space's owners see the requests on the Endpoint's page", async ({ page }) => {
      await stubApi(page, FILLED);
      await page.goto("/projects/helsinki/endpoints/air-internal?lang=en");
      const approve = page.getByRole("button", { name: /Approve/ }).first();
      await expect(approve).toBeVisible();
      await approve.scrollIntoViewIfNeeded();
      await shot(page, "inbox", width);
    });

    test("an owner with nothing to decide reads that", async ({ page }) => {
      await stubApi(page, []);
      await page.goto("/projects/helsinki/endpoints/air-internal?lang=en");
      const empty = page.getByText("Nobody has asked for access to this space.");
      await expect(empty).toBeVisible();
      await empty.scrollIntoViewIfNeeded();
      await shot(page, "inbox-empty", width);
    });
  });
}
