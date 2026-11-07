import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { writeFileSync } from "node:fs";
import { axeViolations } from "./axe";

// "Import rows" on a space type (T-4867): a GeoJSON file's points map to the location NGSI-LD gives
// every entity of a class that specialises ngsi-ld-core's Entity, a file over the limit is refused
// with its size and the limit, and a space whose Policy refuses the person says once that nothing
// was written and whom to ask. `vite preview` has no Portal API behind it, so the API and the space
// surface are answered in the browser. With SHOTS_DIR set, the 1440 px and 400 px screenshots are
// written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const TYPE = "PointOfInterest";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
  status: { phase: "Live" },
});

// helsinki.linkml.yaml's shape: the class lists no geometry of its own.
const LINKML = [
  "id: https://hel.fi/models/helsinki",
  "name: helsinki",
  "imports:",
  "  - linkml:types",
  "  - ngsi-ld-core",
  "classes:",
  "  PointOfInterest:",
  "    is_a: Entity",
  "    slots: [name, serviceCategory, url]",
  "slots:",
  "  name: { range: string }",
  "  serviceCategory: { range: string, required: true }",
  "  url: { range: string }",
  "",
].join("\n");

const GEOJSON = JSON.stringify({
  type: "FeatureCollection",
  features: [
    { type: "Feature", id: "oodi", properties: { name: "Oodi", serviceCategory: "library" }, geometry: { type: "Point", coordinates: [24.9384, 60.1739] } },
    { type: "Feature", id: "yrjonkatu", properties: { name: "Yrjönkatu", serviceCategory: "swimmingHall" }, geometry: { type: "Point", coordinates: [24.9373, 60.1676] } },
  ],
});

async function stubApi(page: Page): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path.endsWith("/datamodels/helsinki/source")) return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    if (path.endsWith("/entityOperations/create")) {
      return json({ type: "https://joinedcontext.com/errors/forbidden", title: "Access Denied by Policy", status: 403, detail: "no policy grants this operation" }, 403);
    }
    if (path.endsWith("/entities")) {
      const row = { id: `urn:ngsi-ld:${TYPE}:hel.fi:${SPACE}:servicemap-1`, type: TYPE, name: { type: "Property", value: "Kallio Library" }, serviceCategory: { type: "Property", value: "library" } };
      return json([row], 200, { "NGSILD-Results-Count": "1" });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/usage`)) return json({ entities: 1, broker: "main", observedAt: "2026-10-21T04:00:00Z" });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki" }));
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml: LINKML })]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`import rows: GeoJSON points to location, the size limit and a refused space in words (${label})`, async ({ page }, info) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    // The grid keeps Import under its toolbar's "More" panel (T-4883).
    await page.locator("summary", { hasText: /^More$/ }).click();
    await page.getByRole("button", { name: "Import rows" }).click();
    const dialog = page.getByRole("dialog", { name: `Import rows into ${TYPE}` });
    const input = dialog.locator("input[type=file]");

    const big = info.outputPath("too-large.csv");
    writeFileSync(big, Buffer.alloc(53_000_000, "a"));
    await input.setInputFiles(big);
    await expect(dialog.getByText("The file is 50.5 MB; an import reads at most 50 MB.", { exact: false })).toBeVisible();
    await shot(page, `import-too-large-${label}`);

    await input.setInputFiles({ name: "places.geojson", mimeType: "application/geo+json", buffer: Buffer.from(GEOJSON) });
    await expect(dialog.getByRole("combobox", { name: "location" })).toHaveValue("location");
    await expect(dialog.getByText("2 rows are ready, none refused.")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `import-geojson-mapped-${label}`);

    await dialog.getByRole("button", { name: "Create 2 rows" }).click();
    await expect(dialog.getByRole("alert")).toContainText(
      `Nothing was written: the space ${SPACE} does not let your role create ${TYPE} entities (no policy grants this operation).`,
    );
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `import-refused-${label}`);
  });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`import rows: a file past 10 000 rows says how many it leaves out (F-UI3-011, ${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    await page.locator("summary", { hasText: /^More$/ }).click();
    await page.getByRole("button", { name: "Import rows" }).click();
    const dialog = page.getByRole("dialog", { name: `Import rows into ${TYPE}` });
    const csv = "name\n" + Array.from({ length: 10_005 }, (_, i) => `Place ${i}`).join("\n") + "\n";
    await dialog.locator("input[type=file]").setInputFiles({ name: "places.csv", mimeType: "text/csv", buffer: Buffer.from(csv) });
    await expect(dialog.getByText(/The file holds 10,?005 rows; only the first 10,?000 are read\./)).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `import-truncated-${label}`);
  });
}

