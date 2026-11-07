import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// A space's "Look inside" as a data workspace (T-4825): it lands on the rows, the entity types
// and their saved views in a left list, the model, quality, retention, endpoints and policies as
// tabs, the broker's count under the rows. `vite preview` has no Portal API behind it, so the API
// and the space surface are answered in the browser. With SHOTS_DIR set, the 1440 px and 400 px
// screenshots are written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki-kpi";
const TYPES = ["KeyPerformanceIndicator", "BikeHireDockingStation"];

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT, ...(title ? { title: { en: title } } : {}) },
  spec,
  status: { phase: "Live" },
});

const LINKML = [
  "id: https://hel.fi/models/kpi",
  "name: helsinki-kpi",
  "classes:",
  "  KeyPerformanceIndicator:",
  "    title: Key performance indicator",
  "    slots: [id, name, kpiValue, category]",
  "  BikeHireDockingStation:",
  "    title: Bike station",
  "    slots: [id, name, availableBikeNumber]",
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name }",
  "  kpiValue: { range: float, title: Value }",
  "  category: { range: string, title: Category }",
  "  availableBikeNumber: { range: integer }",
  "",
].join("\n");

const VIEW = {
  id: "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e01",
  type: TYPES[0],
  kind: "grid",
  mode: "collaborative",
  title: "Over target",
  owner: "jana.kovacova",
  config: { q: "kpiValue>50" },
  version: 1,
  createdAt: "2026-10-20T15:00:00Z",
  updatedAt: "2026-10-20T15:00:00Z",
};

function kpi(n: number) {
  return {
    id: `urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:kpi:${n}`,
    type: TYPES[0],
    name: { type: "Property", value: `District ${n}` },
    kpiValue: { type: "Property", value: 10 * n },
    category: { type: "Property", value: n % 2 ? "quantitative" : "qualitative" },
  };
}

function station(n: number) {
  return {
    id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:station:${n}`,
    type: TYPES[1],
    name: { type: "Property", value: ["Kamppi", "Rautatientori", "Kaivopuisto", "Töölönlahti"][n % 4] },
    availableBikeNumber: { type: "Property", value: 3 + n, observedAt: "2026-10-20T15:55:00Z" },
    freeSlotNumber: { type: "Property", value: 20 - n, observedAt: "2026-10-20T15:55:00Z" },
    status: { type: "Property", value: "working" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93 + n / 100, 60.17] } },
  };
}

// The broker stores each entity in full expanded form with its metadata, several times what its
// keyValues sample weighs: 12 KPIs of ~1.1 KiB and 8 stations of ~2.3 KiB (T-6156).
const KPI_BYTES = 13824;
const STATION_BYTES = 18432;

// How the broker and the gateway answer: a filled space, one that holds nothing yet, a broker that
// fails, and a person the gateway refuses (T-6145).
type Mode = "filled" | "empty" | "error" | "refused" | "loading";

async function stubApi(page: Page, counted = true, mode: Mode = "filled"): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path.endsWith("/datamodels/helsinki-kpi/source")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    }
    if ((path.endsWith("/entities") || path.endsWith(`/spaces/${SPACE}/usage`)) && mode === "loading") {
      // the gateway and the broker still counting: the page shows what it waits for
      return new Promise<void>(() => undefined);
    }
    if (path.endsWith("/entities") && mode === "refused") {
      return json({ type: "about:blank", title: "Forbidden", status: 403, detail: "not readable" }, 403);
    }
    if (path.endsWith("/entities") && mode === "empty") return json([], 200, { "NGSILD-Results-Count": "0" });
    if (path.endsWith("/entities")) {
      const rows =
        url.searchParams.get("type") === TYPES[1]
          ? Array.from({ length: 8 }, (_, i) => station(i + 1))
          : Array.from({ length: 12 }, (_, i) => kpi(i + 1));
      return json(rows, 200, counted ? { "NGSILD-Results-Count": String(rows.length) } : {});
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/usage`) && (mode === "error" || mode === "refused")) {
      const status = mode === "error" ? 502 : 403;
      const detail = mode === "error" ? "the context broker main did not answer" : "you may not read this space's usage";
      return json({ type: "about:blank", title: "Not counted", status, detail }, status);
    }
    if (path.endsWith(`/spaces/${SPACE}/usage`) && mode === "empty") {
      return json({ entities: 0, attrInstances: 0, estimated: false, bytes: { entities: 0, attrInstances: 0, byType: {} }, broker: "main", observedAt: "2026-10-20T16:00:00Z" });
    }
    if (path.endsWith(`/spaces/${SPACE}/usage`)) {
      return json({
        entities: 20,
        attrInstances: 3250000,
        estimated: true,
        bytes: { entities: KPI_BYTES + STATION_BYTES, attrInstances: 1536 * 1024 * 1024, byType: {
          // the broker keys a type by its expanded IRI; the page matches it to the model's name
          "https://smartdatamodels.org/dataModel.KPI/KeyPerformanceIndicator": KPI_BYTES,
          "https://smartdatamodels.org/dataModel.Transportation/BikeHireDockingStation": STATION_BYTES,
        } },
        broker: "main",
        observedAt: "2026-10-20T16:00:00Z",
      });
    }
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [VIEW] });
    if (path.endsWith(`/spaces/${SPACE}`)) {
      return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki-kpi" }, "Helsinki KPIs"));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki-kpi", { contextSpaceRef: SPACE, version: "1.0.0", classes: TYPES, linkml: LINKML })]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([manifest("Endpoint", "kpi-public", { contextSpaceRef: SPACE, slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public", enabledRepresentations: ["ngsi-ld"] })]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`Look inside lands on the rows with the types on the left (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

    const tabs = page.getByRole("tablist", { name: "Parts of the space" });
    await expect(tabs.getByRole("tab", { name: "Data", exact: true })).toHaveAttribute("aria-selected", "true");
    const types = page.getByRole("navigation", { name: "Entity types" });
    if (width >= 1024) {
      await expect(types.locator('button[aria-current="true"]').first()).toHaveAttribute("title", TYPES[0]);
      await expect(types.getByRole("button", { name: "Over target" })).toBeVisible();
    } else {
      // Below lg the types are a picker above the grid (T-4992).
      await expect(types).toBeHidden();
      await expect(page.getByRole("combobox", { name: "Type", exact: true })).toHaveValue(TYPES[0]);
    }
    await expect(page.getByText("District 1", { exact: true })).toBeVisible();
    // The rows count through the grid's own answer; the whole space's scan is on the model tab (T-4885).
    await expect(page.getByTestId("space-total")).toHaveCount(0);
    await expect(page.locator(".jc-grid-total")).toHaveText("12 matching");
    // Nothing wider than the screen: the page body never scrolls sideways (UI-27).
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(sideways).toBeLessThanOrEqual(0);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `look-inside-${label}`);
    await page.locator(".jc-grid-total").scrollIntoViewIfNeeded();
    await shot(page, `look-inside-footer-${label}`);

    await tabs.getByRole("tab", { name: "Data model" }).click();
    await expect(page).toHaveURL(/[?&]tab=model/);
    await expect(page.getByRole("table", { name: "Entity types" })).toBeVisible();
    await expect(page.getByTestId("space-total")).toHaveText("The space holds 20 entities.");
    // T-6085: the history count and both sizes, and the broker's word that it estimated them.
    await expect(page.getByTestId("space-history")).toHaveText("Its history holds 3,250,000 records.");
    await expect(page.getByTestId("space-size")).toHaveText(
      "It takes 1.5 GiB in the broker's store: 32 KiB current state, 1.5 GiB history.",
    );
    await expect(page.getByTestId("space-estimated")).toBeVisible();
    // T-6145: each type's share of the current state, and each sample entity's own size.
    const typeTable = page.getByRole("table", { name: "Entity types" });
    const kpiRow = typeTable.getByRole("row").filter({ hasText: TYPES[0] });
    await expect(kpiRow.getByTitle("13,824 B")).toHaveText("14 KiB");
    const bikeRow = typeTable.getByRole("row").filter({ hasText: TYPES[1] });
    await expect(bikeRow.getByTitle("18,432 B")).toHaveText("18 KiB");
    const sampleBytes = new TextEncoder().encode(JSON.stringify(kpi(1))).length;
    // districts 1 and 3 weigh the same; the samples sit under the type at every width (T-6156)
    await expect(kpiRow.getByTitle(`Size of the values shown, as JSON: ${sampleBytes} B`).first()).toHaveText(`Size: ${sampleBytes} B`);
    await expect(kpiRow.getByText("District 1", { exact: false }).first()).toBeVisible();
    await expect(bikeRow.getByRole("list", { name: "Sample entities" })).toContainText("Rautatientori");
    await expect(page.getByText(/^Size is what a type's entities take/)).toBeVisible();
    await shot(page, `model-tab-${label}`);
    await page.getByTestId("space-total").scrollIntoViewIfNeeded();
    await shot(page, `model-tab-total-${label}`);
  });
}

// T-6145: the model tab when the space holds nothing, the broker fails, or the person is refused:
// no size the broker did not give, and the reason in words.
for (const mode of ["empty", "error", "refused", "loading"] as const) {
  for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
    test(`The model tab says what it knows of a ${mode} space (${label})`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await stubApi(page, true, mode);
      await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&tab=model`);
      const table = page.getByRole("table", { name: "Entity types" });
      await expect(table).toBeVisible();
      const row = table.getByRole("row").filter({ hasText: TYPES[0] });
      if (mode === "empty") {
        // the samples sit under the type, a phone sees them too (T-6156)
        await expect(row.getByText("No entity of this type yet.")).toBeVisible();
        await expect(page.getByTestId("space-total")).toHaveText("The space holds 0 entities.");
      } else if (mode === "refused") {
        // Jana is signed in: the refusal names the missing grant in one cell, no count, no size (T-6156)
        await expect(row.getByText("No read grant: your account may not read this type through the endpoint.")).toBeVisible();
        await expect(row.locator("td")).toHaveCount(2);
        await expect(page.getByText("you may not read this space's usage")).toBeVisible();
      } else if (mode === "loading") {
        await expect(row.getByText("Loading", { exact: false }).first()).toBeVisible();
      } else {
        await expect(page.getByText("the context broker main did not answer")).toBeVisible();
      }
      // no type size and no space size the broker did not give
      if (mode !== "refused") await expect(row.locator("td").nth(2)).toHaveText("—");
      if (mode !== "empty") await expect(page.getByTestId("space-size")).toHaveCount(0);
      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(sideways).toBeLessThanOrEqual(0);
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("table", { name: "Entity types" }).scrollIntoViewIfNeeded();
      await shot(page, `model-tab-${mode}-${label}`);
    });
  }
}

// T-4885: an answer the gateway narrowed carries no total (R22); the footer says so in words.
for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`A grid answer without a total says so (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, false);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    // The paged footer states the endpoint's own count or nothing: no number it guessed (R22).
    const footer = page.locator(".jc-grid-page-info");
    await expect(footer).toBeVisible();
    await expect(page.locator(".jc-grid-total")).toHaveCount(0);
    await footer.scrollIntoViewIfNeeded();
    await expect(page.getByTestId("space-total")).toHaveCount(0);
    await shot(page, `grid-no-total-${label}`);
  });
}
