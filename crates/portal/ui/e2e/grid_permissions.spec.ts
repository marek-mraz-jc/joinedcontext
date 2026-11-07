import { writeFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// The permission-aware grid in the explorer (T-5530): locked columns say why, an attribute the
// person may not read is not drawn at all and no value of it is fetched, the review reads
// names and formatted values, and export and delete are offered only where the server allows
// them. `vite preview` has no gateway, so its answers are stubbed in the browser; the explorer,
// the grid and its review are the real thing.

const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const TYPE = "BikeHireDockingStation";
const SHOTS = process.env.SHOTS_DIR ?? "test-results/grid-permissions";
const MANY = 10_000;

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: unknown) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki", title: { en: name } },
  spec,
  status: { phase: "Live" },
});

const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, name, availableBikeNumber, status, location, refPointOfInterest, operatorNote]",
  "  PointOfInterest:",
  "    slots: [id, name]",
  "slots:",
  "  id: {}",
  "  name: { range: string }",
  "  availableBikeNumber: { range: integer }",
  "  status: { range: string }",
  "  location: { range: GeoProperty }",
  "  refPointOfInterest: { range: PointOfInterest }",
  "  operatorNote: { range: string }",
  "",
].join("\n");

type Persona = "editor" | "viewer";

/** Each persona's decisions, as the gateway's `access/attributes` answers them. */
function decisions(persona: Persona) {
  const row = (name: string, read: boolean, write: boolean, reason: string, del = false) => ({ name, read, write, delete: del, reason });
  const editor = persona === "editor";
  return [
    row("name", true, editor, editor ? "grant" : "not_in_grant", editor),
    row("availableBikeNumber", true, false, "not_in_grant", editor),
    row("status", true, false, "prohibited", editor),
    row("location", true, editor, editor ? "grant" : "not_in_grant", editor),
    row("refPointOfInterest", true, editor, editor ? "grant" : "not_in_grant", editor),
    row("operatorNote", false, false, "not_in_grant"),
  ];
}

function station(at: number) {
  const n = String(at + 1).padStart(5, "0");
  return {
    id: `urn:ngsi-ld:${TYPE}:hel.fi:mobility:${n}`,
    type: TYPE,
    name: { type: "Property", value: `Station ${n}` },
    availableBikeNumber: { type: "Property", value: at % 17 },
    status: { type: "Property", value: "working" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.94 + (at % 100) / 1000, 60.17] } },
    refPointOfInterest: { type: "Relationship", object: `urn:ngsi-ld:PointOfInterest:hel.fi:poi:${n}` },
  };
}

/** What the endpoint's own page reads for its data section: the published JSON Schema. */
const JSON_SCHEMA = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  $defs: {
    [TYPE]: {
      type: "object",
      properties: {
        name: { type: "string" },
        availableBikeNumber: { type: "integer" },
        status: { type: "string" },
        operatorNote: { type: "string" },
      },
    },
  },
};

interface Stubbed {
  /** Every `attrs` the entity reads asked for, to prove a hidden attribute is never requested. */
  attrs: (string | null)[];
}

async function stub(page: Page, persona: Persona, rows: number): Promise<Stubbed> {
  const seen: Stubbed = { attrs: [] };
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: `${persona}@hel.fi`, name: persona, roles: [] });
    if (path.endsWith(`/api/endpoint/${SLUG}/access/attributes`)) return json({ type: TYPE, attributes: decisions(persona) });
    if (path.endsWith("/access/check")) return json({ decision: persona === "editor" });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as unknown[]).map(() => ({ decision: persona === "editor" })) });
    if (path.endsWith("/access")) return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity"], attributes: "*" }], prohibitions: [] });
    if (path.endsWith(`/endpoints/helsinki-bikes`)) {
      return json(manifest("Endpoint", "helsinki-bikes", { contextSpaceRef: "mobility", slug: SLUG, audience: "project-list", enabledRepresentations: ["ngsi-ld", "csv"] }));
    }
    if (path.endsWith("/schema/index.json")) return json({ models: [{ name: "helsinki-mobility", version: 1 }] });
    if (path.endsWith("/json-schema")) return json(JSON_SCHEMA);
    if (path.endsWith("/entities") && url.searchParams.get("type") === "PointOfInterest") {
      const poi = (n: string, name: string) => ({ id: `urn:ngsi-ld:PointOfInterest:hel.fi:poi:${n}`, type: "PointOfInterest", name: { type: "Property", value: name } });
      return json([poi("kallio", "Kallio library"), poi("kamppi", "Kamppi centre")]);
    }
    if (path.endsWith("/entities")) {
      seen.attrs.push(url.searchParams.get("attrs"));
      const offset = Number(url.searchParams.get("offset") ?? "0");
      const limit = Number(url.searchParams.get("limit") ?? "20");
      const page = Array.from({ length: Math.max(0, Math.min(limit, rows - offset)) }, (_, at) => station(offset + at));
      return route.fulfill({ status: 200, contentType: "application/json", headers: { "NGSILD-Results-Count": String(rows) }, body: JSON.stringify(page) });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) return json(list([manifest("ContextSpace", "mobility", { dataModelRef: "helsinki-mobility" })]));
    if (path.endsWith("/endpoints")) return json(list([manifest("Endpoint", "helsinki-bikes", { contextSpaceRef: "mobility", slug: SLUG, audience: "project-list" })]));
    if (path.endsWith("/datamodels")) return json(list([manifest("DataModel", "helsinki-mobility", { version: "1.0.0", classes: [TYPE], linkml: LINKML })]));
    return json(list([]));
  });
  return seen;
}

const EXPLORE = `/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&attrs=name,availableBikeNumber,status,location,refPointOfInterest&lang=en`;

test("10 000 rows: the first row and twenty screens of scrolling, timed", async ({ page }) => {
  test.setTimeout(120_000);
  await stub(page, "editor", MANY);
  const started = Date.now();
  await page.goto(EXPLORE);
  await expect(page.getByRole("button", { name: "Open: Station 00001" })).toBeVisible();
  const firstRow = Date.now() - started;
  const box = page.locator(".jc-grid-scroll").first();
  const scrollStart = Date.now();
  for (let step = 1; step <= 20; step++) {
    await box.evaluate((el, by) => el.scrollBy(0, by), 1500);
    await page.waitForTimeout(0);
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  }
  await expect(page.getByRole("button", { name: /^Open: Station \d{5}$/ }).first()).toBeVisible();
  const scroll = Date.now() - scrollStart;
  const line = `${new Date().toISOString()} ${process.env.TIMING_LABEL ?? "run"} firstRowMs=${firstRow} scroll20Ms=${scroll}\n`;
  await writeFile(`${SHOTS}-timings.txt`, line, { flag: "a" });
  console.log(line.trim());
});


/** Opens the explorer as `persona` on three stations and waits for the first row. */
async function explore(page: Page, persona: Persona): Promise<Stubbed> {
  const seen = await stub(page, persona, 3);
  await page.goto(EXPLORE);
  await expect(page.getByRole("button", { name: "Open: Station 00001" })).toBeVisible();
  // The shots are of the table, not of the filters above it.
  await page.getByRole("grid").evaluate((grid) => grid.scrollIntoView({ block: "start" }));
  return seen;
}

const lock = (page: Page, reason: string) => page.getByRole("img", { name: `Read-only: ${reason}` });

for (const width of [1440, 400]) {
  test.describe(`the permission-aware grid at ${width} px`, () => {
    test.use({ viewport: { width, height: width === 400 ? 860 : 900 } });

    test("an editor sees locked and editable columns side by side", async ({ page }) => {
      await explore(page, "editor");
      await expect(page.getByRole("textbox", { name: "Edit name" }).first()).toBeVisible();
      await expect(lock(page, "Not in your access.")).toBeVisible();
      await expect(lock(page, "Explicitly not allowed by a policy.")).toBeAttached();
      await page.screenshot({ path: `${SHOTS}/editor-mixed-${width}.png` });
    });

    // T-6210: a grid that edits nothing wears no lock; the values are simply values.
    test("a viewer sees no editor and no lock", async ({ page }) => {
      await explore(page, "viewer");
      await expect(page.getByRole("grid")).toBeVisible();
      await expect(lock(page, "Not in your access.")).toHaveCount(0);
      await expect(page.getByRole("textbox", { name: "Edit name" })).toHaveCount(0);
      await page.screenshot({ path: `${SHOTS}/viewer-locked-${width}.png` });
    });

    test("an attribute the person may not read is not drawn and never asked for (T-6210)", async ({ page }) => {
      const seen = await explore(page, "editor");
      await expect(page.getByRole("columnheader", { name: /ID/ }).first()).toBeVisible();
      await expect(page.getByRole("columnheader", { name: /operatorNote/ })).toHaveCount(0);
      await expect(page.getByText("hidden: no access")).toHaveCount(0);
      expect(seen.attrs.length).toBeGreaterThan(0);
      for (const attrs of seen.attrs) expect(attrs ?? "").not.toContain("operatorNote");
      await page.screenshot({ path: `${SHOTS}/hidden-column-${width}.png` });
    });

    test("the review names the row and the column and formats each value", async ({ page }) => {
      await explore(page, "editor");
      await page.getByRole("textbox", { name: "Edit name" }).first().fill("Station 00001 north");
      // A GeoProperty and a Relationship change are reviewed in sdk/e2e/grid_review.spec.ts: no
      // Portal grid edits a geometry in place.
      await page.getByRole("button", { name: "Review the changes" }).click();
      const review = page.getByRole("region", { name: "Review the changes" }).first();
      await expect(review).toContainText("Station 00001");
      const text = (await review.textContent()) ?? "";
      expect(text).not.toContain("urn:");
      expect(text).not.toContain("[object Object]");
      await review.scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${SHOTS}/review-${width}.png` });
    });

    test("a paste over locked cells counts them as not allowed", async ({ page }) => {
      await explore(page, "editor");
      const name = page.getByRole("textbox", { name: "Edit name" }).first();
      await name.focus();
      await page.keyboard.press("Escape");
      await page.evaluate(() => {
        const grid = document.querySelector('[role="grid"]');
        const data = new DataTransfer();
        data.setData("text/plain", "Station 1 renamed\t7\tbroken\n");
        grid?.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
      });
      await expect(page.getByText(/1 cells pasted, 2 not allowed by your access/)).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/paste-skipped-${width}.png` });
    });

    test("delete waits for the endpoint's own decision and says why it is not offered", async ({ page }) => {
      await explore(page, "viewer");
      await page.getByRole("button", { name: "Open: Station 00001" }).click();
      const remove = page.getByTestId("explore-delete");
      await expect(remove).toHaveAttribute("aria-disabled", "true");
      await remove.scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${SHOTS}/delete-refused-${width}.png` });
    });

    test("the CSV download is not offered when a shown column cannot be read", async ({ page }) => {
      await stub(page, "editor", 3);
      await page.goto("/projects/helsinki/endpoints/helsinki-bikes?lang=en");
      const csv = page.getByRole("button", { name: "Download these rows as CSV" });
      await expect(csv).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByText(/does not let you read Operator note/i).first()).toBeVisible();
      await csv.scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${SHOTS}/export-refused-${width}.png` });
    });
  });
}
