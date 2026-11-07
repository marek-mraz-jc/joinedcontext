import { expect, test } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import type { Page } from "@playwright/test";

// The journey over entity data (T-1437; UI-64, UI-67, UI-59). `vite preview` has no portal API and
// no gateway behind it, so both are answered in the browser: the grid, the explorer around it and
// the endpoint page are the real thing, and what travels to the endpoint is asserted from the
// requests themselves.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const TYPE = "BikeHireDockingStation";
const EMPTY_DOCK = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:001`;
const FULL_DOCK = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:002`;
/** In the space and not in this endpoint: the comparison marks it (T-1435). */
const HIDDEN_DOCK = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:003`;

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

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility" } },
    spec: { dataModelRef: "helsinki-mobility", isSandbox: false },
    status: { phase: "Live" },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name: "helsinki-bikes",
      namespace: "helsinki",
      title: { en: "Helsinki city bike stations" },
    },
    spec: {
      contextSpaceRef: "mobility",
      slug: SLUG,
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "csv"],
    },
    status: { phase: "Live" },
  },
]);

/** The model inline, so the slots the edit needs stand between nothing and the page. */
const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, availableBikeNumber, name]",
  "slots:",
  "  id: {}",
  "  availableBikeNumber: { range: integer, minimum_value: 0 }",
  "  name: { range: string }",
  "",
].join("\n");

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-mobility", namespace: "helsinki" },
    spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML, artifacts: { jsonSchema: "./helsinki-mobility.v1.schema.json" } },
  },
]);

/** What `GET /api/endpoint/{slug}/schema/v{n}/json-schema` publishes, for the endpoint's own page. */
const JSON_SCHEMA = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  $defs: {
    [TYPE]: {
      type: "object",
      properties: { availableBikeNumber: { type: "integer" }, name: { type: "string" } },
    },
  },
};

function dock(id: string, bikes: number, name: string) {
  return {
    id,
    type: TYPE,
    availableBikeNumber: {
      type: "Property",
      value: bikes,
      unitCode: "C62",
      observedAt: "2026-09-19T08:00:00Z",
    },
    name: { type: "Property", value: name },
  };
}

const HISTORY = {
  id: EMPTY_DOCK,
  type: TYPE,
  availableBikeNumber: [
    { type: "Property", value: 3, observedAt: "2026-09-19T06:00:00Z" },
    { type: "Property", value: 0, observedAt: "2026-09-19T08:00:00Z" },
  ],
};

/** A grant that lets this person correct the one attribute, and one that only reads. */
function grant(write: boolean) {
  return {
    permissions: [
      {
        resource: { type: TYPE },
        actions: write ? ["queryEntity", "updateAttrs"] : ["queryEntity"],
        attributes: ["availableBikeNumber", "name"],
      },
    ],
    prohibitions: [],
  };
}

interface Stub {
  /** Every non-GET call to the endpoint: `PATCH <path> <body>`. */
  writes: string[];
  /** The query string of every entity read, so what the filter asked for is asserted, not guessed. */
  reads: string[];
}

async function stubApi(page: Page, options: { write?: boolean } = {}): Promise<Stub> {
  const writes: string[] = [];
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({
        status,
        contentType: "application/json",
        headers,
        body: JSON.stringify(body),
      });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    // The access panel asks the endpoint's own PDP every action in one batch, before any read.
    if (path.endsWith("/access/v1/evaluations")) {
      const asked = JSON.parse(request.postData() ?? "{}") as { evaluations: unknown[] };
      return json({ evaluations: asked.evaluations.map(() => ({ decision: options.write === true })) });
    }
    if (path.endsWith("/access/check")) {
      return json({ decision: options.write === true });
    }
    if (path.endsWith("/access/attributes")) {
      return json(attributeTable(TYPE, ["availableBikeNumber", "name"], options.write === true ? ["availableBikeNumber", "name"] : []));
    }
    if (path.endsWith("/access")) {
      return json(grant(options.write === true));
    }
    if (request.method() !== "GET") {
      // The reason travels with each write (DM-113), so it is kept beside the body.
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""} reason=${request.headers()["jc-reason"] ?? ""}`);
      return route.fulfill({ status: 204, body: "" });
    }
    if (path.includes("/temporal/entities/")) {
      return json(HISTORY);
    }
    if (path.endsWith("/schema/index.json")) {
      return json({ models: [{ name: "helsinki-mobility", version: 1 }] });
    }
    if (path.endsWith("/json-schema")) {
      return json(JSON_SCHEMA);
    }
    if (path.endsWith("/entities")) {
      reads.push(url.search);
      // The space holds one dock more than this endpoint answers, and the filter of the question
      // narrows the endpoint's own page to the empty one.
      const q = url.searchParams.get("q") ?? "";
      const docks = path.startsWith("/cs/")
        ? [dock(EMPTY_DOCK, 0, "Rautatientori"), dock(FULL_DOCK, 7, "Kamppi"), dock(HIDDEN_DOCK, 2, "Töölö")]
        : q.includes("availableBikeNumber==0")
          ? [dock(EMPTY_DOCK, 0, "Rautatientori")]
          : [dock(EMPTY_DOCK, 0, "Rautatientori"), dock(FULL_DOCK, 7, "Kamppi")];
      return json(docks, 200, { "NGSILD-Results-Count": String(docks.length) });
    }
    if (path === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/endpoints/helsinki-bikes")) {
      return json(ENDPOINTS.items[0]);
    }
    if (path.endsWith("/datamodels")) {
      return json(MODELS);
    }
    return json(list([]));
  });
  return { writes, reads };
}

/** Opens a column's menu and its settings, where History and the metadata columns are (T-4827). */
async function openColumnDetails(page: Page, column: string): Promise<void> {
  await page.getByRole("button", { name: `Column options: ${column}` }).click();
  await page.getByRole("dialog", { name: `Column options: ${column}` }).getByText("Column settings").click();
}

/**
 * The control `label` names, reached with the Tab key alone: a person who never touches a pointer
 * has to arrive at the cell they are correcting (UI-67). The accessible name is read from the
 * focused element itself, so nothing here depends on how many stops lie between.
 */
async function tabTo(page: Page, label: string): Promise<void> {
  for (let stop = 0; stop < 80; stop += 1) {
    const focused = await page.evaluate(() => {
      const active = document.activeElement as HTMLInputElement | null;
      if (!active) {
        return "";
      }
      // A radio or a checkbox carries its name in the label around it, and a button in its text.
      return (
        active.getAttribute("aria-label") ??
        active.labels?.[0]?.textContent?.trim() ??
        active.textContent?.trim() ??
        ""
      );
    });
    if (focused === label) {
      return;
    }
    await page.keyboard.press("Tab");
  }
  throw new Error(`no control named "${label}" is reachable with the Tab key`);
}

const EXPLORE = "/projects/helsinki/explore";

test.describe("the entity grid", () => {
  test("opens on the endpoint and the filter the assistant handed over (UI-59)", async ({
    page,
  }) => {
    const { reads } = await stubApi(page);

    // What `jc_ui_navigate` builds for the page kind `entities`: an endpoint, a type and the
    // question's own filter, and no space — the page finds the endpoint's own.
    await page.goto(
      `${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&q=${encodeURIComponent("availableBikeNumber==0")}&lang=en`,
    );

    await expect(page.getByLabel("Context space")).toHaveValue("mobility");
    await expect(page.getByLabel("Endpoint", { exact: true })).toHaveValue("helsinki-bikes");
    await expect(page.getByRole("button", { name: "Open: Rautatientori" }).first()).toBeVisible();
    await expect(page.getByRole("button", { name: "Open: Kamppi" })).toHaveCount(0);
    expect(reads.some((search) => search.includes("q=availableBikeNumber%3D%3D0"))).toBe(true);
  });

  test("narrows a column and shows when a value was observed", async ({ page }) => {
    const { reads } = await stubApi(page);

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: "Open: Kamppi" }).first()).toBeVisible();

    // The grid's own filter row, on the column: the operator and then the value.
    await page.getByLabel("Filter: availableBikeNumber").selectOption("equals");
    await page.getByLabel("Value: availableBikeNumber").fill("0");

    await expect(page.getByRole("button", { name: "Open: Kamppi" })).toHaveCount(0);
    await expect
      .poll(() => reads.filter((search) => search.includes("availableBikeNumber%3D%3D0")).length)
      .toBeGreaterThan(0);

    // When the value was observed is metadata of the same column, switched on from its menu.
    // The attribute's metadata sits under Details in the column's own menu (T-4827).
    await openColumnDetails(page, "availableBikeNumber");
    await page.getByRole("checkbox", { name: "Observed" }).check();
    await expect(
      page.getByRole("columnheader", { name: "availableBikeNumber · Observed" }),
    ).toBeVisible();
  });

  // T-3253: the export is the view, the grid's own filter included. Since T-6107 the Endpoint
  // serves the file itself, so the filter travels in the link of its Export menu.
  test("exports the filtered view as the Endpoint's CSV file, the grid's filter included", async ({ page }) => {
    await stubApi(page);

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: "Open: Kamppi" }).first()).toBeVisible();
    await page.getByLabel("Filter: availableBikeNumber").selectOption("equals");
    await page.getByLabel("Value: availableBikeNumber").fill("0");
    await expect(page.getByRole("button", { name: "Open: Kamppi" })).toHaveCount(0);

    await page.locator("summary", { hasText: "Export" }).click();
    const href = new URL((await page.getByRole("link", { name: "CSV", exact: true }).getAttribute("href"))!, "http://x");
    expect(href.pathname).toBe(`/api/endpoint/${SLUG}/file.csv`);
    expect(href.searchParams.get("q")).toContain("availableBikeNumber==0");
  });

  test("corrects a value with the keyboard alone and applies it through the endpoint", async ({
    page,
  }) => {
    const { writes } = await stubApi(page, { write: true });

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    const cell = page.getByLabel("Edit availableBikeNumber").first();
    await expect(cell).toBeVisible();

    await page.keyboard.press("Tab");
    await tabTo(page, "Edit availableBikeNumber");
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("4");
    await expect(cell).toHaveValue("4");

    await tabTo(page, "Review the changes");
    await page.keyboard.press("Enter");
    const review = page.getByRole("region", { name: "Review the changes" });
    await expect(review).toContainText("Rautatientori");
    // The corrected value was observed now, not when the old reading was (UI-67). A radio group is
    // one Tab stop on the chosen one, and the arrow keys move inside it.
    await tabTo(page, "keep when each value was observed");
    await page.keyboard.press("ArrowDown");
    await expect(
      review.getByRole("radio", { name: "these values were observed now" }),
    ).toBeChecked();
    await tabTo(page, "Apply");
    await page.keyboard.press("Enter");

    await expect.poll(() => writes.length).toBe(1);
    expect(writes[0]).toContain(`PATCH /api/endpoint/${SLUG}/ngsi-ld/v1/entities/`);
    expect(writes[0]).toContain('"availableBikeNumber":{"type":"Property","value":4');
    expect(writes[0]).toContain('"observedAt"');
  });

  test("opens the history of one attribute of one entity", async ({ page }) => {
    await stubApi(page);

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: "Open: Rautatientori" }).first()).toBeVisible();

    // The attribute's metadata sits under Details in the column's own menu (T-4827).
    await openColumnDetails(page, "availableBikeNumber");
    await page.getByRole("button", { name: "History" }).click();

    const history = page.getByRole("dialog").or(page.getByRole("region", { name: "History" }));
    // The panel writes the time in the page's language (T-2991); the instant is its `datetime`.
    await expect(history.locator('time[datetime="2026-09-19T06:00:00Z"]')).toBeVisible();
    await expect(history.getByRole("cell", { name: "3", exact: true })).toBeVisible();
  });

  test("compares the endpoint's page with the space behind it", async ({ page }) => {
    await stubApi(page);

    // The list has no "View data" since T-6095: the data is on the endpoint's own page.
    await page.goto("/projects/helsinki/endpoints/helsinki-bikes?lang=en");
    await page.getByRole("button", { name: "Compare with the space" }).click();

    const compare = page.getByRole("region", { name: "The space / This endpoint" });
    await expect(compare.getByRole("heading", { name: "The space" })).toBeVisible();
    await expect(compare.getByRole("heading", { name: "This endpoint" })).toBeVisible();
    // The dock the endpoint's filter or policy leaves out is marked on the space's side, where a
    // person hovers the row to read why it is there and not here.
    await expect(
      compare.locator('tr[title="not in this endpoint (filter or policy)"]'),
    ).toHaveCount(1);
  });

  test("offers a viewer no edit at all", async ({ page }) => {
    const { writes } = await stubApi(page, { write: false });

    await page.goto(`${EXPLORE}?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await expect(page.getByRole("button", { name: "Open: Rautatientori" }).first()).toBeVisible();

    await expect(page.getByLabel("Edit availableBikeNumber")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Review the changes" })).toHaveCount(0);
    expect(writes).toEqual([]);
  });
});

// T-4826: the space's own grid, as Baserow edits a table: click a cell, type over it, apply with a
// reason, Undo. The grant decides who gets an editor; the gateway decides each write.
const SPACE_PAGE = "/projects/helsinki/spaces/mobility?lang=en";
const SHOTS = process.env.SHOTS_DIR ?? "test-results";

/** The space record and its side reads, which the explorer's stub does not answer. */
async function stubSpace(page: Page, refuse?: string): Promise<void> {
  await page.route("**/api/v1/projects/helsinki/spaces/mobility**", (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/usage")) return json({ entities: 3, broker: "main", observedAt: "2026-10-20T16:00:00Z" });
    if (path.endsWith("/views")) return json({ items: [] });
    if (path.endsWith("/mobility")) return json(SPACES.items[0]);
    return json(list([]));
  });
  if (refuse) {
    // The Policy refuses this one entity: a 403 problem, as the gateway answers it.
    await page.route(`**/api/endpoint/${SLUG}/ngsi-ld/v1/entities/${encodeURIComponent(refuse)}/attrs`, (route) =>
      route.fulfill({
        status: 403,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "https://uri.etsi.org/ngsi-ld/errors/Forbidden", title: "Forbidden", detail: "your role may not write this dock" }),
      }),
    );
  }
}

for (const width of [1440, 400]) {
  test.describe(`the space grid at ${width} px`, () => {
    test.use({ viewport: { width, height: width > 600 ? 900 : 860 } });

    test("edits a value inline, sends the reason, and undoes it", async ({ page }) => {
      const { writes } = await stubApi(page, { write: true });
      await stubSpace(page);
      await page.goto(SPACE_PAGE);
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      const cell = page.getByLabel("Edit Available bike number").first();
      await expect(cell).toHaveValue("0");

      // Click the cell, type over it, Enter keeps the value and goes one row down. At 400 px the
      // pinned id covers the left edge, so the cell is scrolled to the middle first, as a person
      // swipes the table before tapping.
      await cell.evaluate((element) => element.scrollIntoView({ block: "center", inline: "center" }));
      await cell.click({ position: { x: 8, y: 8 } });
      await page.keyboard.press("ControlOrMeta+a");
      await page.keyboard.type("4");
      await page.keyboard.press("Enter");
      await expect(cell).toHaveValue("4");
      await page.getByRole("button", { name: "Review the changes" }).click();
      const review = page.getByRole("region", { name: "Review the changes" });
      await review.getByRole("textbox", { name: "Why you change these values" }).fill("Counter reset at 06:00");
      await page.screenshot({ path: `${SHOTS}/t4826-edit-steward-${width}.png` });
      await review.getByRole("button", { name: "Apply" }).click();

      await expect.poll(() => writes.length).toBe(1);
      expect(writes[0]).toContain(`PATCH /api/endpoint/${SLUG}/ngsi-ld/v1/entities/${encodeURIComponent(EMPTY_DOCK)}/attrs`);
      expect(writes[0]).toContain('"availableBikeNumber":{"type":"Property","value":4');
      expect(writes[0]).toContain(`reason=${encodeURIComponent("Counter reset at 06:00")}`);

      const undo = page.getByRole("button", { name: "Undo" });
      await expect(undo).toBeVisible();
      await undo.scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${SHOTS}/t4826-undo-steward-${width}.png` });
      await undo.click();
      await expect.poll(() => writes.length).toBe(2);
      expect(writes[1]).toContain('"availableBikeNumber":{"type":"Property","value":0');
      expect(writes[1]).toContain(`reason=${encodeURIComponent("Undo: Counter reset at 06:00")}`);
      await expect(page.getByText("The change was undone.")).toBeVisible();
    });

    test("shows the gateway's refusal at the cell", async ({ page }) => {
      await stubApi(page, { write: true });
      await stubSpace(page, FULL_DOCK);
      await page.goto(SPACE_PAGE);
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      const cell = page.getByLabel("Edit Available bike number").nth(1);
      await expect(cell).toHaveValue("7");
      await cell.fill("6");
      await page.getByRole("button", { name: "Review the changes" }).click();
      await page.getByRole("region", { name: "Review the changes" }).getByRole("button", { name: "Apply" }).click();
      await expect(cell).toHaveAttribute("aria-invalid", "true");
      await expect(page.getByRole("gridcell").getByText("your role may not write this dock")).toBeVisible();
      await cell.scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${SHOTS}/t4826-refused-steward-${width}.png` });
    });

    test("gives a viewer no editor", async ({ page }) => {
      const { writes } = await stubApi(page, { write: false });
      await stubSpace(page);
      await page.goto(SPACE_PAGE);
      await expect(page.getByRole("grid")).toBeVisible();
      await expect(page.getByRole("button", { name: "Open: Rautatientori" }).first()).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/t6209-load-${width}.png` });
      // Edit asks the endpoint (T-6209) and says why nothing became editable.
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await expect(page.getByRole("status").filter({ hasText: "No endpoint of this space lets you correct" })).toBeVisible();
      await expect(page.getByLabel("Edit Available bike number")).toHaveCount(0);
      await page.screenshot({ path: `${SHOTS}/t4826-viewer-${width}.png` });
      expect(writes).toEqual([]);
    });
  });
}
