import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The explorer's map (T-3256): a type whose model gives it a location opens on a Map tab beside
// the table, reads the view through the endpoint with the table's own filter, and draws it.
// `vite preview` has no API and no gateway, so both are answered in the browser.

const TYPE = "BikeHireDockingStation";
const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, availableBikeNumber, name, location]",
  "slots:",
  "  id: {}",
  "  availableBikeNumber: { range: integer }",
  "  name: { range: string }",
  "  location:",
  "    range: string",
  "    annotations:",
  "      ngsi_ld_kind: GeoProperty",
  "",
].join("\n");

function dock(n: number, bikes: number, lon: number, lat: number) {
  return {
    id: `urn:ngsi-ld:${TYPE}:hel.fi:mobility:${String(n).padStart(3, "0")}`,
    type: TYPE,
    availableBikeNumber: bikes,
    name: `Dock ${n}`,
    location: { type: "Point", coordinates: [lon, lat] },
  };
}

type Data = "points" | "polygons" | "empty" | "many" | "failing" | "unreported" | "allZero";

// T-5061: a square around a point, for a type whose places are areas.
function area(n: number, lon: number, lat: number) {
  const d = 0.004;
  return { ...dock(n, n % 5, lon, lat), location: { type: "Polygon", coordinates: [[[lon, lat], [lon + d, lat], [lon + d, lat + d], [lon, lat + d], [lon, lat]]] } };
}

// 12 000 docks spread over Helsinki, as the Endpoint would page them; an answer with `georel`
// holds only those inside the asked polygon, at most `limit`.
const MANY = 12_000;
const spread = (i: number) => dock(i + 1, i % 5, 24.7 + (i % 120) * 0.004, 60.1 + Math.floor(i / 120) * 0.002);

function entitiesAnswer(data: Data, url: URL): { status: number; rows: unknown[]; count?: number } {
  const limit = Number(url.searchParams.get("limit") ?? 20);
  const offset = Number(url.searchParams.get("offset") ?? 0);
  // The map's reads are the export's pages of 500; the table's own page still answers.
  if (data === "failing" && limit >= 500) return { status: 502, rows: [] };
  if (data === "empty") return { status: 200, rows: [], count: 0 };
  if (data === "unreported") {
    const rows = Array.from({ length: 24 }, (_, i) => {
      const { availableBikeNumber, ...station } = dock(i + 1, i % 9, 24.9 + (i % 6) * 0.012 + ((i * 7) % 5) * 0.003, 60.15 + Math.floor(i / 6) * 0.012 + ((i * 3) % 4) * 0.002);
      return i % 4 === 3 ? station : { ...station, availableBikeNumber };
    });
    return { status: 200, rows: rows.slice(offset, offset + limit), count: rows.length };
  }
  if (data === "allZero") {
    const rows = Array.from({ length: 6 }, (_, i) => dock(i + 1, 0, 24.9 + i * 0.01, 60.15 + (i % 2) * 0.01));
    return { status: 200, rows: rows.slice(offset, offset + limit), count: rows.length };
  }
  if (data === "polygons") {
    const rows = Array.from({ length: 30 }, (_, i) => area(i + 1, 24.9 + (i % 6) * 0.01, 60.15 + Math.floor(i / 6) * 0.01));
    return { status: 200, rows: rows.slice(offset, offset + limit), count: rows.length };
  }
  if (data === "many") {
    const coordinates = url.searchParams.get("coordinates");
    if (url.searchParams.get("georel") === "within" && coordinates) {
      const ring = (JSON.parse(coordinates) as number[][][])[0];
      const [w, s, e, n] = [ring[0][0], ring[0][1], ring[2][0], ring[2][1]];
      const inside: unknown[] = [];
      for (let i = 0; i < MANY; i += 1) {
        const [lon, lat] = spread(i).location.coordinates;
        if (lon >= w && lon <= e && lat >= s && lat <= n) inside.push(spread(i));
      }
      return { status: 200, rows: inside.slice(offset, offset + limit), count: inside.length };
    }
    return { status: 200, rows: Array.from({ length: Math.max(0, Math.min(limit, MANY - offset)) }, (_, i) => spread(offset + i)), count: MANY };
  }
  const docks = Array.from({ length: 40 }, (_, i) => dock(i + 1, i % 5, 24.9 + (i % 8) * 0.01, 60.15 + Math.floor(i / 8) * 0.01));
  const q = url.searchParams.get("q") ?? "";
  const shown = q.includes("availableBikeNumber==0") ? docks.filter((d) => d.availableBikeNumber === 0) : docks;
  return { status: 200, rows: shown, count: shown.length };
}

async function stub(page: Page, data: Data = "points"): Promise<string[]> {
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, headers: Record<string, string> = {}) =>
      route.fulfill({ status: 200, contentType: "application/json", headers, body: JSON.stringify(body) });
    const path = url.pathname;
    // No basemap here: the map draws on its plain ground, as on an installation without one.
    if (path.includes("/basemap/")) return route.fulfill({ status: 404, body: "" });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "jana", name: "Jana Kováčová", roles: [] });
    if (path.endsWith("/access/check")) return json({ decision: false });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as unknown[]).map(() => ({ decision: false })) });
    if (path.endsWith("/access/attributes")) return json(attributeTable(TYPE, ["availableBikeNumber", "name", "location"]));
    if (path.endsWith("/access")) return json({ permissions: [{ action: "queryEntity", resource: { type: TYPE }, attributes: "*" }], prohibitions: [] });
    if (path.endsWith("/entities")) {
      reads.push(url.search);
      const answer = entitiesAnswer(data, url);
      if (answer.status !== 200) {
        return route.fulfill({ status: answer.status, contentType: "application/json", body: JSON.stringify({ title: "Bad Gateway", status: answer.status }) });
      }
      return json(answer.rows, answer.count === undefined ? {} : { "NGSILD-Results-Count": String(answer.count) });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility" } }, spec: { dataModelRef: "helsinki-mobility" }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "helsinki-bikes", namespace: "helsinki", title: { en: "Helsinki city bike stations" } }, spec: { contextSpaceRef: "mobility", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld"] }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "helsinki-mobility", namespace: "helsinki" }, spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML } }]));
    }
    return json(list([]));
  });
  return reads;
}

test("a located type opens on the map with the view's filter", async ({ page }) => {
  const reads = await stub(page);
  await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&q=${encodeURIComponent("availableBikeNumber==0")}&lang=en`);
  await expect(page.locator("tbody tr").first()).toBeVisible();

  await page.getByRole("tab", { name: "Map" }).click();
  await expect(page.getByRole("status").filter({ hasText: "on the map" })).toHaveText("8 of 8 on the map");
  await expect(page.getByRole("application", { name: `Map of ${TYPE}` })).toBeVisible();
  expect(reads.some((search) => search.includes("availableBikeNumber%3D%3D0") && search.includes("attrs=location"))).toBe(true);
  expect(await axeViolations(page)).toEqual([]);

  // Back to the table, which waited where it was.
  await page.getByRole("tab", { name: "Entities" }).click();
  await expect(page.locator("tbody tr")).toHaveCount(8);
});

// T-6442: the assistant's "map the stations" hand-off opens the map itself, coloured, with the
// stations that report no number counted and grey.
for (const width of [1440, 400]) {
  test(`the assistant's map hand-off draws the coloured view at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await stub(page, "unreported");
    await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&colorBy=availableBikeNumber&view=map&lang=en`);
    await expect(page.getByRole("tab", { name: "Map" })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByRole("status").filter({ hasText: "on the map" })).toHaveText(
      "24 of 24 on the map. Colour shows Available bike number from 0 to 8, yellow to red. Grey: no value (6).",
      { timeout: 30_000 },
    );
    await expect(page.getByRole("application", { name: `Map of ${TYPE}` })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    if (process.env.JC_EVIDENCE_DIR) {
      mkdirSync(process.env.JC_EVIDENCE_DIR, { recursive: true });
      await page.waitForTimeout(800);
      await page.screenshot({ path: `${process.env.JC_EVIDENCE_DIR}/map-handoff-${width}.png` });
    }
  });
}

// T-6442: every shown value equal (one point, or the assistant's own `availableBikeNumber==0`): no
// ramp to draw, and MapLibre refuses one, so the stations must still reach the canvas.
test("a coloured map whose values are all equal still draws its points", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.text().includes("ascending")) errors.push(message.text());
  });
  await stub(page, "allZero");
  await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&colorBy=availableBikeNumber&view=map&lang=en`);
  await expect(page.getByRole("status").filter({ hasText: "on the map" })).toContainText("All points show Available bike number 0", { timeout: 30_000 });
  await expect(page.getByText("all the same: 0", { exact: true })).toBeVisible();
  // The one-value colour, read from the pixels of the canvas: the map says nothing when it fails.
  const orange = async () => {
    const shot = await page.getByRole("application", { name: `Map of ${TYPE}` }).locator("canvas").screenshot();
    return page.evaluate(async (base64) => {
      const image = new Image();
      image.src = `data:image/png;base64,${base64}`;
      await image.decode();
      const canvas = new OffscreenCanvas(image.width, image.height);
      const context = canvas.getContext("2d") as OffscreenCanvasRenderingContext2D;
      context.drawImage(image, 0, 0);
      const { data } = context.getImageData(0, 0, image.width, image.height);
      let found = 0;
      for (let i = 0; i < data.length; i += 4) {
        if (Math.abs(data[i] - 0xfd) < 8 && Math.abs(data[i + 1] - 0x8d) < 8 && Math.abs(data[i + 2] - 0x3c) < 8) found += 1;
      }
      return found;
    }, shot.toString("base64"));
  };
  await expect.poll(orange, { timeout: 15_000 }).toBeGreaterThan(50);
  expect(errors).toEqual([]);
  if (process.env.JC_EVIDENCE_DIR) {
    mkdirSync(process.env.JC_EVIDENCE_DIR, { recursive: true });
    await page.mouse.wheel(0, -2000);
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${process.env.JC_EVIDENCE_DIR}/map-all-equal-1440.png` });
  }
});

// T-3257: a number charted from the explorer as a histogram, with its axis in words, ready to save.
test("charts an attribute of the view as the histogram its numbers suggest", async ({ page }) => {
  await stub(page);
  await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.getByRole("button", { name: "Chart", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: `Chart an attribute of ${TYPE}` });
  await dialog.getByLabel("Attribute").selectOption("availableBikeNumber");
  await expect(dialog.getByRole("radio", { name: "Histogram" })).toBeChecked();
  await expect(dialog.getByText("Number of entities in each range of availableBikeNumber.")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Save to the dashboard" })).toBeEnabled();
  expect(await axeViolations(page)).toEqual([]);
});

// T-5061: the map at both widths and both themes over points, areas, nothing, 12 000 entities and
// an Endpoint that answers 502. Every state leaves a picture for the UX read in the task.
const SHOTS = process.env.T5061_SHOTS ?? "test-results/explore-map";
mkdirSync(SHOTS, { recursive: true });
const STATUS: Record<Exclude<Data, "unreported" | "allZero">, RegExp> = {
  points: /^40 of 40 on the map$/,
  polygons: /^30 of 30 on the map$/,
  empty: /^No entity of this view has a location; the Entities tab lists them all\.$/,
  many: /in this view, 12,000 in all. More than 5,000 (match|lie in this view)/,
  failing: /./,
};
for (const data of ["points", "polygons", "empty", "many", "failing"] as const) {
  for (const width of [1440, 400]) {
    for (const scheme of ["light", "dark"] as const) {
      test(`the map of ${data} at ${width} px, ${scheme}`, async ({ page }) => {
        test.slow(data === "many");
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await page.emulateMedia({ colorScheme: scheme });
        const sent: string[] = [];
        page.on("request", (request) => {
          if (request.url().includes("/entities")) sent.push(request.headers().authorization ?? "");
        });
        const reads = await stub(page, data);
        await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
        await page.getByRole("tab", { name: "Map" }).click();
        if (data === "failing") {
          await expect(page.getByRole("button", { name: "Retry" })).toBeVisible({ timeout: 30_000 });
        } else {
          const status = page.getByRole("status").filter({ hasText: /on the map|location|in this view/ });
          await expect(status).toHaveText(STATUS[data], { timeout: 30_000 });
          // Nothing located draws no empty map: the sentence says where the entities are.
          await expect(page.getByRole("application", { name: `Map of ${TYPE}` })).toHaveCount(data === "empty" ? 0 : 1);
        }
        if (data === "many") {
          // The first read stops at 5 000; the view the map fits is read again as a polygon.
          expect(reads.some((search) => search.includes("georel=within") && search.includes("geometry=Polygon"))).toBe(true);
          expect(reads.filter((search) => !search.includes("georel")).length).toBeGreaterThanOrEqual(5);
        }
        expect(sent.every((value) => value === "")).toBe(true);
        expect(await axeViolations(page)).toEqual([]);
        await page.screenshot({ path: `${SHOTS}/explore-map-${data}-${scheme}-${width}.png`, fullPage: true });
      });
    }
  }
}
