import { expect, test } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The data workspace's states T-5064 did not shoot (T-5093): the form labelled by the model, the
// dark theme, a 10 000-row type, two sessions on one cell, an Endpoint answering 502, and the empty
// space; plus the one toolbar row of a non-grid view and the footer's quiet "Summarize". `vite
// preview` has no Portal API behind it, so the API and the space surface are answered in the
// browser. With SHOTS_DIR set, the 1440 px and 400 px screenshots go there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const TYPE = "Event";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

// Helsinki's seed shape: the slots carry no title, the class labels them in slot_usage, and the
// status is an enum whose values carry their labels as annotations.
const LINKML = [
  "id: https://hel.fi/models/events",
  "name: helsinki-events",
  "classes:",
  "  Event:",
  "    slots: [name, startDate, eventStatus, audience]",
  "    slot_usage:",
  "      name: { annotations: { label: { tag: label, value: { en: Name, fi: Nimi } } } }",
  "      startDate: { annotations: { label: { tag: label, value: { en: Starts, fi: Alkaa } } } }",
  "      eventStatus: { annotations: { label: { tag: label, value: { en: Status, fi: Tila } } } }",
  "      audience: { annotations: { label: { tag: label, value: { en: Places, fi: Paikat } } } }",
  "slots:",
  "  name: { range: string }",
  "  startDate: { range: datetime }",
  "  eventStatus: { range: EventStatus }",
  "  audience: { range: integer }",
  "enums:",
  "  EventStatus:",
  "    permissible_values:",
  "      EventScheduled: { annotations: { label: { tag: label, value: { en: Scheduled } } } }",
  "      EventCancelled: { annotations: { label: { tag: label, value: { en: Cancelled } } } }",
  "      EventPostponed: { annotations: { label: { tag: label, value: { en: Postponed } } } }",
  "",
].join("\n");

const NAMES = ["Workshop for Families", "Finnish Language Café", "Board game club", "Men's cafe", "Time machine"];

function event(n: number, audience = 10 * n) {
  return {
    id: `urn:ngsi-ld:Event:hel.fi:linkedevents:${n}`,
    type: TYPE,
    name: { type: "Property", value: `${NAMES[n % NAMES.length]} ${n}` },
    startDate: { type: "Property", value: `2026-10-${String(21 + (n % 7)).padStart(2, "0")}T17:00:00Z` },
    eventStatus: { type: "Property", value: n % 5 ? "EventScheduled" : "EventCancelled" },
    audience: { type: "Property", value: audience },
  };
}

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
  status: { phase: "Live" },
});

interface Stub {
  /** How many rows the type holds; the rows are made per page. */
  total?: number;
  /** The space surface's status for a read, such as an Endpoint answering 502. */
  readStatus?: number;
  /** The space has no model: no type at all. */
  noTypes?: boolean;
  /** The grant writes `audience`. */
  writer?: boolean;
  /** Resolved when the other session's change is to arrive on the live stream. */
  liveGate?: Promise<void>;
  /** The other session's value of row 1's audience, after the gate. */
  changed?: { audience: number };
  patched?: unknown[];
}

async function stubApi(page: Page, stub: Stub): Promise<void> {
  let changedNow = false;
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const method = route.request().method();
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path.endsWith("/datamodels/helsinki-events/source")) return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    if (path.endsWith("/live")) {
      if (stub.liveGate && !changedNow) {
        await stub.liveGate;
        changedNow = true;
        return route.fulfill({ status: 200, contentType: "text/event-stream", body: 'retry: 600000\nevent: changed\ndata: {"type":"Event"}\n\n' });
      }
      return route.fulfill({ status: 200, contentType: "text/event-stream", body: "retry: 600000\n\n" });
    }
    if (path.endsWith("/attrs") && method === "PATCH") {
      stub.patched?.push(route.request().postDataJSON());
      return route.fulfill({ status: 204 });
    }
    if (path.endsWith("/entities") && method === "POST") {
      return json({ type: "https://uri.etsi.org/ngsi-ld/errors/Forbidden", title: "Forbidden", detail: "no policy grants this operation" }, 403);
    }
    if (path.endsWith("/entities")) {
      if (stub.readStatus) {
        return json({ type: "https://uri.etsi.org/ngsi-ld/errors/InternalError", title: "Bad Gateway", detail: "the broker did not answer" }, stub.readStatus);
      }
      const total = stub.total ?? 24;
      const offset = Number(url.searchParams.get("offset") ?? 0);
      const limit = Number(url.searchParams.get("limit") ?? 25);
      const rows = Array.from({ length: Math.max(0, Math.min(limit, total - offset)) }, (_, i) => {
        const n = offset + i + 1;
        return n === 1 && changedNow && stub.changed ? event(n, stub.changed.audience) : event(n);
      });
      return json(rows, 200, { "NGSILD-Results-Count": String(total) });
    }
    if (path.endsWith("/access/attributes")) {
      return json(attributeTable(TYPE, ["name", "startDate", "eventStatus", "audience"], stub.writer ? ["audience"] : []));
    }
    if (path.endsWith("/access")) {
      return json({
        permissions: [
          { resource: { type: TYPE }, actions: ["queryEntity"], attributes: "*" },
          ...(stub.writer ? [{ resource: { type: TYPE }, actions: ["updateAttrs"], attributes: ["audience"] }] : []),
        ],
      });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) return json(manifest("ContextSpace", SPACE, stub.noTypes ? {} : { dataModelRef: "helsinki-events" }));
    if (path.endsWith("/datamodels")) {
      return json(list(stub.noTypes ? [] : [manifest("DataModel", "helsinki-events", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml: LINKML, artifacts: { jsonSchema: "./helsinki-events.v1.schema.json" } })]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([manifest("Endpoint", "app-helsinki-data", { contextSpaceRef: SPACE, slug: SLUG, audience: "project", enabledRepresentations: ["ngsi-ld", "csv"] })]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

async function noSideways(page: Page): Promise<void> {
  const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(sideways).toBeLessThanOrEqual(0);
}

const open = (page: Page, view = "grid") => page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&view=${view}`);

test.describe("the data workspace on a touch phone", () => {
  test.use({ viewport: { width: 400, height: 860 }, hasTouch: true, isMobile: true });

  test("gives every toolbar control a finger's height and keeps the first rows on the screen", async ({ page }) => {
    await stubApi(page, {});
    await open(page);
    await expect(page.getByText("Workshop for Families 5").first()).toBeVisible();
    const toolbar = page.getByTestId("grid-toolbar");
    const tools = toolbar.locator("summary", { hasText: "Tools" });
    expect((await tools.boundingBox())?.height).toBeGreaterThanOrEqual(44);
    expect((await toolbar.getByRole("button", { name: "Add row" }).boundingBox())?.height).toBeGreaterThanOrEqual(44);
    await shot(page, "grid-toolbar-touch-400");
    await tools.tap();
    await toolbar.locator("summary", { hasText: "Sort" }).tap();
    await expect(toolbar.getByRole("button", { name: "Close" })).toBeInViewport();
    await shot(page, "grid-tools-open-touch-400");
    await toolbar.getByRole("button", { name: "Close" }).tap();
    await expect(toolbar.locator("summary", { hasText: "Sort" })).toBeHidden();
  });
});

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test.describe(`the data workspace matrix (${label})`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height });
    });

    test("labels the form and Add row by the model, and asks the status from its enum", async ({ page }) => {
      await stubApi(page, {});
      await open(page);
      await expect(page.getByText("Workshop for Families 5").first()).toBeVisible();
      // The footer is a quiet Summarize under each column, not a "None" box.
      await expect(page.getByRole("combobox", { name: "Footer of Places" })).toHaveValue("none");
      await expect(page.getByRole("combobox", { name: "Footer of Places" }).locator("option:checked")).toHaveText("Summarize");
      await page.getByRole("button", { name: "Add row" }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByLabel("Starts")).toBeVisible();
      await expect(dialog.getByText("startDate", { exact: true })).toHaveCount(0);
      const status = dialog.getByRole("combobox", { name: "Status" });
      await expect(status.locator("option")).toContainText(["Scheduled", "Cancelled", "Postponed"]);
      await noSideways(page);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `form-labels-${label}`);
    });

    test("keeps the refusal and Create of Add row on the screen", async ({ page }) => {
      await stubApi(page, {});
      await open(page);
      await page.getByRole("button", { name: "Add row" }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: "Name" }).fill("Harbour walk");
      await dialog.getByRole("button", { name: "Create", exact: true }).click();
      const refusal = dialog.getByRole("alert");
      await expect(refusal).toContainText("Ask a project owner");
      await expect(refusal).toBeInViewport({ ratio: 1 });
      await expect(dialog.getByRole("button", { name: "Create", exact: true })).toBeInViewport({ ratio: 1 });
      await noSideways(page);
      await shot(page, `add-row-refused-${label}`);
    });

    test("puts the first row near the top with the tools in one menu", async ({ page }) => {
      await stubApi(page, {});
      await open(page);
      await expect(page.getByText("Workshop for Families 5").first()).toBeVisible();
      const toolbar = page.getByTestId("grid-toolbar");
      await shot(page, `grid-toolbar-${label}`);
      if (width >= 640) {
        await expect(toolbar.locator("summary", { hasText: "Tools" })).toHaveCount(0);
        return;
      }
      await expect(toolbar.locator("summary", { hasText: "Sort" })).toBeHidden();
      const top = await toolbar.boundingBox();
      const row = await page.locator(".jc-grid-table tbody tr").first().boundingBox();
      // About 330 px before T-6463: three tool rows, the search and the Filters block.
      expect((row?.y ?? 0) - (top?.y ?? 0)).toBeLessThan(230);
      await toolbar.locator("summary", { hasText: "Tools" }).click();
      await toolbar.locator("summary", { hasText: "Sort" }).click();
      await expect(toolbar.locator("summary", { hasText: "Row height" })).toBeInViewport();
      await noSideways(page);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `grid-tools-open-${label}`);
    });

    test("gives every API call a copy button", async ({ page }) => {
      await stubApi(page, {});
      await open(page, "api");
      const calls = page.getByTestId("api-calls");
      for (const call of ["List", "Count", "One entity", "History"]) {
        await expect(calls.getByRole("button", { name: `Copy the ${call} URL` })).toBeVisible();
      }
      await noSideways(page);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `api-${label}`);
    });

    test("keeps a non-grid view's actions in the one toolbar row", async ({ page }) => {
      await stubApi(page, {});
      await open(page, "gallery");
      const toolbar = page.getByTestId("grid-toolbar");
      await expect(toolbar.getByRole("button", { name: "Add row" })).toBeVisible();
      await expect(toolbar.locator("summary", { hasText: "More" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Import rows" })).toBeHidden();
      await noSideways(page);
      await shot(page, `gallery-toolbar-${label}`);
    });

    test("reads in the dark theme, the query folded behind its toggle", async ({ page }) => {
      await page.emulateMedia({ colorScheme: "dark" });
      await stubApi(page, {});
      await open(page);
      await expect(page.getByText("Workshop for Families 5").first()).toBeVisible();
      const query = page.getByRole("status", { name: "The query this asks" });
      await expect(query).toHaveCount(0);
      if (width < 640) await page.getByTestId("grid-toolbar").locator("summary", { hasText: "Tools" }).click();
      await page.getByRole("button", { name: "Query", exact: true }).click();
      // Every row is asked for, so the line says so and offers nothing to copy (0373b9ea).
      await expect(query).toHaveText("all records");
      await expect(page.getByRole("button", { name: "Copy the query" })).toHaveCount(0);
      await shot(page, `dark-${label}`);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("counts a 10 000-row type from the endpoint and loads it a page at a time", async ({ page }) => {
      await stubApi(page, { total: 10_000 });
      await open(page);
      await expect(page.locator(".jc-grid-total")).toContainText("10000 matching");
      await noSideways(page);
      await shot(page, `ten-thousand-${label}`);
    });

    test("shows the other session's write to a cell while this one edits it", async ({ page }) => {
      let release = () => {};
      const liveGate = new Promise<void>((resolve) => {
        release = resolve;
      });
      const patched: unknown[] = [];
      await stubApi(page, { writer: true, liveGate, changed: { audience: 999 }, patched });
      await open(page);
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      const cell = page.getByRole("spinbutton", { name: /Places/ }).first();
      await cell.fill("15");
      release();
      // The other session's value reaches the grid; this session's typed value is still pending.
      await expect(page.locator("tr.jc-grid-tr--info")).toHaveCount(1);
      await shot(page, `conflict-live-${label}`);
      await page.getByRole("button", { name: "Review the changes" }).click();
      // The review reads the value the other session wrote as Before: applying overwrites it knowingly.
      const review = page.getByRole("region", { name: "Review the changes" });
      await expect(review.getByRole("row").nth(1)).toContainText("999");
      await expect(review.getByRole("row").nth(1)).toContainText("15");
      await review.scrollIntoViewIfNeeded();
      await shot(page, `conflict-review-${label}`);
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await expect.poll(() => patched.length).toBe(1);
    });

    test("names an Endpoint's 502 instead of an empty grid", async ({ page }) => {
      await stubApi(page, { readStatus: 502 });
      await open(page);
      const alert = page.getByRole("alert").filter({ hasText: "The space's data did not answer" });
      await expect(alert).toContainText("HTTP 502");
      await expect(alert.getByRole("button", { name: "Retry" })).toBeVisible();
      await expect(page.getByText("You can see this space through its endpoints")).toHaveCount(0);
      await noSideways(page);
      await shot(page, `endpoint-502-${label}`);
    });

    test("says what to do in a space with no rows and in one with no type", async ({ page }) => {
      await stubApi(page, { total: 0 });
      await open(page);
      await expect(page.locator(".jc-grid-total")).toContainText("0 matching");
      await shot(page, `empty-type-${label}`);
      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stubApi(page, { noTypes: true });
      await open(page);
      await expect(page.getByText(/no type|first rows|CSV/i).first()).toBeVisible();
      await noSideways(page);
      await shot(page, `empty-space-${label}`);
    });
  });
}
