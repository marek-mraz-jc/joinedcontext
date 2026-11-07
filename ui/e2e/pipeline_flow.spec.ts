import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-0643: Pipeline flow canvas e2e journey (UI-49, PL-43, PL-44).
// Validates dragging palette stage onto SVG canvas, inline bloblang editing,
// stage test trace mapping (error state on compute node), fixing mapping and reaching green state.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const BRANDING = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Helsinki",
  organisation: "City of Helsinki",
  orgDomain: "hel.fi",
  domain: "portal.hel.fi",
  contactEmail: "data@hel.fi",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const DATASOURCES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "mqtt-city", namespace: "helsinki" },
    spec: { type: "mqtt", mqtt: { urls: ["tls://mqtt.hel.fi:8883"], topics: ["aq/#"] } },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "helsinki" },
    spec: { contextSpaceRef: "air", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-flow-1234", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

async function stubApi(page: Page, getTestTrace: () => unknown) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json(BRANDING);
    }
    if (path.endsWith("/pipelines/test")) {
      return json(getTestTrace());
    }
    if (request.method() !== "GET") {
      return json(CHANGE, 202);
    }
    if (path === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (path.endsWith("/datasources")) {
      return json(DATASOURCES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    return json(list([]));
  });
}

test.describe("pipeline flow canvas", () => {
  test("drags stage to flow canvas, reflects test trace error and green states", async ({ page }) => {
    let currentTrace: unknown = {
      input: { events: 1, bytes: 64, sample: { id: "1" } },
      mapping: [],
      validation: [],
      errors: [{ stage: "mapping", line: 1, message: "unexpected token" }],
    };

    await stubApi(page, () => currentTrace);

    await page.goto("/projects/helsinki/pipelines?lang=en");
    // An empty list offers the action twice: in the page header and in the empty state
    // itself (UI-01). The header's is the one a person with pipelines also has.
    await page.getByRole("button", { name: "New pipeline" }).first().click();

    const dialog = page.getByTestId("form-page");
    await expect(dialog).toBeVisible();
    // The flow is the advanced editor's, folded under the workbench (T-2709).
    await dialog.getByText("Advanced editor").click();

    const paletteBloblang = page.getByTestId("palette-bloblang");
    const canvas = page.getByTestId("flow-canvas");

    await expect(paletteBloblang).toBeVisible();
    await expect(canvas).toBeVisible();

    // Drag compute stage from palette onto SVG canvas. Both ends in view first: a drag that has
    // to scroll the page on its way is dropped by Chromium, which a one-line hint above the
    // studio was enough to cause (T-2899).
    await canvas.scrollIntoViewIfNeeded();
    await expect(paletteBloblang).toBeInViewport();
    await paletteBloblang.dragTo(canvas);

    const computeNode = page.getByTestId("flow-node-compute");
    await expect(computeNode).toBeVisible();

    // Provide a sample file for testing
    await dialog.getByTestId("pipeline-studio").locator('input[type="file"]').setInputFiles({
      name: "sample.json",
      mimeType: "application/json",
      buffer: Buffer.from(JSON.stringify([{ id: "sensor-01", pm10: 42 }])),
    });

    // Enter bad mapping in flow bloblang editor
    const bloblangArea = page.getByTestId("flow-bloblang");
    await expect(bloblangArea).toBeVisible();
    await bloblangArea.fill("root = invalid_syntax {");

    // Run studio test
    const testButton = page.getByRole("button", { name: "Test mapping" });
    await expect(testButton).toBeEnabled();
    await testButton.click();

    // Compute node turns red (error state)
    await expect(computeNode).toHaveAttribute("data-state", "error");

    // Correct mapping and mock green trace
    currentTrace = {
      input: { events: 1, bytes: 64, sample: { id: "sensor-01", pm10: 42 } },
      mapping: [{ id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:sensor-01", type: "AirQualityObserved" }],
      validation: [{ index: 0, ok: true, problems: [] }],
      errors: [],
    };

    await bloblangArea.fill('root = this\nroot.type = "AirQualityObserved"');
    await testButton.click();

    // Compute node reflects green ok state
    await expect(computeNode).toHaveAttribute("data-state", "ok");
  });
  // T-3221 (PL-56, PL-68): built by drag and wire, no YAML. A processor found by search and the
  // compute kind are dragged onto the canvas, a wire from the source's port puts the compute step
  // at the head of the lane, the list view shows that order, undo takes the wire back, and a test
  // run paints every node green.
  test("builds a pipeline by dragging nodes and drawing a wire, and its test run is green", async ({ page }) => {
    const trace = {
      input: { events: 1, bytes: 64, sample: { id: "sensor-01", pm10: 42 } },
      mapping: [{ id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:sensor-01", type: "AirQualityObserved" }],
      validation: [{ index: 0, ok: true, problems: [] }],
      errors: [],
    };
    await stubApi(page, () => trace);
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByText("Advanced editor").click();
    const canvas = page.getByTestId("flow-canvas");
    await canvas.scrollIntoViewIfNeeded();

    // A node says what it lacks before any test.
    await expect(page.getByTestId("flow-node-state-source")).toHaveText("Needs a source");

    await page.getByLabel("Find a processor").fill("log");
    const found = page.getByTestId("palette-found-log");
    await found.scrollIntoViewIfNeeded();
    await found.dragTo(canvas);
    await expect(page.getByTestId("flow-node-step-0")).toBeVisible();
    await page.getByTestId("palette-bloblang").dragTo(canvas);
    await expect(page.getByTestId("flow-node-compute")).toBeVisible();

    // The lane runs log, then compute; a wire from the source into compute puts it first.
    await page.getByTestId("flow-port-out-source").dragTo(page.getByTestId("flow-port-in-compute"));
    await expect(page.getByTestId("flow-said")).toHaveText("Step moved: the wire set the order the steps run in.");
    await page.getByRole("button", { name: "Show as a list" }).click();
    const items = page.getByRole("list", { name: "The pipeline's nodes in the order they run" }).getByRole("listitem");
    await expect(items.nth(1)).toContainText("bloblang");
    await expect(items.nth(2)).toContainText("log");
    await page.getByRole("button", { name: "Undo" }).click();
    await expect(items.nth(1)).toContainText("log");
    await page.getByRole("button", { name: "Redo" }).click();
    await expect(items.nth(1)).toContainText("bloblang");
    await page.getByRole("button", { name: "Show as a list" }).click();

    await dialog.getByTestId("pipeline-studio").locator('input[type="file"]').setInputFiles({
      name: "sample.json",
      mimeType: "application/json",
      buffer: Buffer.from(JSON.stringify([{ id: "sensor-01", pm10: 42 }])),
    });
    await page.getByTestId("flow-bloblang").fill('root = this\nroot.type = "AirQualityObserved"');
    await page.getByRole("button", { name: "Test mapping" }).click();
    await expect(page.getByTestId("flow-node-compute")).toHaveAttribute("data-state", "ok");
  });
  // T-3222: a deliberately broken step is found from the graph alone: the node turns red with the
  // error in words, its drawer shows the input that broke it, and a Debug node on the wire into it
  // lists what crossed that wire.
  test("finds the broken step from the graph: red node, its error and the input that broke it", async ({ page }) => {
    const trace = {
      input: { events: 2, bytes: 40, sample: { id: "s1", pm10: 4 } },
      mapping: [],
      validation: [],
      errors: [{ stage: "mapping", step: 1, message: "expected number, got string" }],
      stages: [
        { step: 0, reached: 2, sample: { id: "s2", pm10: "many" } },
        { step: 1, reached: 2 },
      ],
    };
    await stubApi(page, () => trace);
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByText("Advanced editor").click();
    const canvas = page.getByTestId("flow-canvas");
    await canvas.scrollIntoViewIfNeeded();
    await page.getByLabel("Find a processor").fill("log");
    await page.getByTestId("palette-found-log").click();
    await page.getByTestId("palette-bloblang").click();

    await dialog.getByTestId("pipeline-studio").locator('input[type="file"]').setInputFiles({
      name: "sample.json",
      mimeType: "application/json",
      buffer: Buffer.from(JSON.stringify([{ id: "s1", pm10: 4 }, { id: "s2", pm10: "many" }])),
    });
    await page.getByTestId("flow-bloblang").fill("root = this\nroot.pm10 = this.pm10.number()");
    await page.getByTestId("flow-node-compute").click();
    await page.getByTestId("palette-debug").click();
    await page.getByRole("button", { name: "Test mapping" }).click();

    const compute = page.getByTestId("flow-node-compute");
    await expect(compute).toHaveAttribute("data-state", "error");
    await expect(compute).toContainText("expected number, got string");
    await expect(page.getByTestId("flow-wire-count-step-0>compute")).toHaveText("2");
    await page.getByTestId("flow-wire-step-0>compute").click({ force: true });
    // The breaking input reads as a record, field beside value (T-5516), not as JSON text.
    const input = page.getByTestId("flow-breaking-input");
    await expect(input.getByRole("term").filter({ hasText: /^pm10$/ })).toBeVisible();
    await expect(input.getByRole("definition").filter({ hasText: /^many$/ })).toBeVisible();
    const logged = page.getByTestId("flow-debug").getByRole("listitem").first();
    await expect(logged.getByRole("term").filter({ hasText: /^pm10$/ })).toBeVisible();
    await expect(logged.getByRole("definition").filter({ hasText: /^many$/ })).toBeVisible();
  });
});
