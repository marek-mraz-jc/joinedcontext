import { expect, test } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { smallTouchTargets } from "./touch";

// The space grid shaped like Baserow (T-4883): one toolbar row, row numbers with checkboxes, one
// line a row with the name frozen, a filter panel whose AND/OR conditions reach the endpoint as
// NGSI-LD `q`, a search that asks the endpoint, footer counts the endpoint makes, and a bulk
// delete. `vite preview` has no Portal API behind it, so the API and the space surface are
// answered in the browser, which also records every `q` the grid sent. With SHOTS_DIR set, the
// 1440 px and 400 px screenshots of each state are written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const TYPE = "Event";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT, ...(title ? { title: { en: title } } : {}) },
  spec,
  status: { phase: "Live" },
});

const LINKML = [
  "id: https://hel.fi/models/events",
  "name: helsinki-events",
  "classes:",
  "  Event:",
  "    title: Event",
  "    slots: [id, name, address, description, startDate, audience]",
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name }",
  "  address: { range: string, title: Address }",
  "  description: { range: string, title: Description }",
  "  startDate: { range: datetime, title: Starts }",
  "  audience: { range: integer, title: Places }",
  "",
].join("\n");

const NAMES = [
  "Liiketoimintasuunnitelman laskelmat - video for founders who plan their first year",
  "Workshop for Families",
  "Finnish Language Café at Iso Omena Library",
  "Board game club",
  "Toastmasters - become a better speaker",
  "Men's cafe",
  "Chinese Mid Autumn Festival",
  "Time machine",
];

function event(n: number) {
  return {
    id: `urn:ngsi-ld:Event:hel.fi:linkedevents:${n}`,
    type: TYPE,
    name: { type: "Property", value: NAMES[n % NAMES.length] },
    address: { type: "Property", value: `Leppävaarankatu ${n}, 02600 Espoo` },
    description: { type: "Property", value: "A long description of what happens at the event, who it is for and how to come along." },
    startDate: { type: "Property", value: `2026-10-${String(21 + (n % 7)).padStart(2, "0")}T17:00:00Z` },
    ...(n % 4 ? { audience: { type: "Property", value: 10 * n } } : {}),
  };
}

// The bulk tools' model adds a status enum, which "Colour by field" and "Set a field" pick from.
const LINKML_STATUS = LINKML.replace("audience]", "audience, status]")
  .replace("slots:\n  id", "enums:\n  EventStatus:\n    permissible_values: { open: {}, cancelled: {} }\nslots:\n  id")
  .concat("  status: { range: EventStatus, title: Status }\n");

interface Recorded {
  asked: string[];
  deleted: string[];
  patched: { id: string; body: unknown; reason: string | null }[];
  /** Live changes the space's event stream has sent. */
  live: number;
}

async function stubApi(page: Page, asked: string[], deleted: string[], extra?: { linkml: string; seen: Recorded }): Promise<void> {
  const linkml = extra?.linkml ?? LINKML;
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const method = route.request().method();
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path.endsWith("/datamodels/helsinki-events/source")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: linkml });
    }
    if (extra && path.endsWith("/attrs") && method === "PATCH") {
      // The Policy refuses the value on the second row: the dialog names it, the others land.
      const id = decodeURIComponent(path.split("/entities/")[1]?.replace(/\/attrs$/, "") ?? "");
      if (id.endsWith(":2")) return json({ title: "Forbidden", detail: "the Policy does not grant status on this row" }, 403);
      extra.seen.patched.push({ id, body: route.request().postDataJSON(), reason: route.request().headers()["jc-reason"] ?? null });
      return route.fulfill({ status: 204 });
    }
    if (extra && path.endsWith("/live")) {
      // The change comes after the rows are on the screen, as one made by someone else would.
      if (extra.seen.live === 0) await page.getByText("Workshop for Families").first().waitFor();
      extra.seen.live += 1;
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: extra.seen.live === 1 ? 'retry: 600000\nevent: changed\ndata: {"type":"Event"}\n\n' : "retry: 600000\n\n",
      });
    }
    if (path.includes("/entities/") && method === "DELETE") {
      // The Policy refuses the second row: the bar names it and keeps it.
      const id = decodeURIComponent(path.split("/entities/")[1] ?? "");
      if (id.endsWith(":2")) return json({ title: "Forbidden", detail: "the Policy does not grant this delete" }, 403);
      deleted.push(id);
      return route.fulfill({ status: 204 });
    }
    if (path.endsWith("/entities")) {
      const q = url.searchParams.get("q") ?? "";
      asked.push(q);
      // After a live change the third row holds another name: the grid flashes it.
      const rows = Array.from({ length: 24 }, (_, i) => event(i + 1)).map((row, i) =>
        extra && extra.seen.live > 0 && i === 2 ? { ...row, name: { type: "Property", value: "Renamed while you looked" } } : row,
      );
      const total = q.endsWith(";audience") || q === "audience" ? 18 : 24;
      return json(rows, 200, { "NGSILD-Results-Count": String(total) });
    }
    if (path.endsWith("/access/attributes")) {
      const slots = ["name", "address", "description", "startDate", "audience", "status"];
      return json(attributeTable(TYPE, slots, slots));
    }
    if (path.endsWith("/access")) {
      return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity", "updateAttrs", "deleteEntity"], attributes: "*" }] });
    }
    if (path.endsWith("/trash") && method === "POST") return json({ id: "c0ffee00-0000-4000-8000-000000000001" }, 201);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) {
      return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki-events" }, "Helsinki city context"));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki-events", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml, artifacts: { jsonSchema: "./helsinki-events.v1.schema.json" } })]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([manifest("Endpoint", "app-helsinki-data", { contextSpaceRef: SPACE, slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project", enabledRepresentations: ["ngsi-ld", "csv"] })]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

/** Opens a toolbar tool by its label; a phone holds them in one Tools menu (T-6463). */
async function openTool(page: Page, label: string): Promise<void> {
  const toolbar = page.getByTestId("grid-toolbar");
  const item = toolbar.locator("summary", { hasText: label });
  if (!(await item.isVisible())) await toolbar.locator("summary", { hasText: "Tools" }).click();
  await item.click();
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`The space grid works like Baserow (${label})`, async ({ page }) => {
    const asked: string[] = [];
    const deleted: string[] = [];
    await page.setViewportSize({ width, height });
    await stubApi(page, asked, deleted);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

    // One toolbar row, the grid's own per-column filter row under the header (grid rework), numbered rows of one line.
    const toolbar = page.getByTestId("grid-toolbar");
    // A phone folds the tools into one Tools menu and drops the filter row (T-6463).
    const phone = width < 640;
    await expect(phone ? toolbar.locator("summary", { hasText: "Tools" }) : toolbar.getByText("Filter", { exact: true })).toBeVisible();
    await expect(page.getByText("Workshop for Families").first()).toBeVisible();
    await expect(page.locator(".jc-grid-filter-row")).toHaveCount(phone ? 0 : 1);
    await expect(page.locator(".jc-grid")).toHaveAttribute("data-row-size", "medium");
    // The row-number column holds the row's checkbox since the grid rework: no number is drawn.
    await expect(page.locator("tbody td.jc-grid-rownum").first().getByRole("checkbox", { name: /^Select row: / })).toBeVisible();
    await expect(page.locator("tbody .jc-grid-rownum-n")).toHaveCount(0);
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(sideways).toBeLessThanOrEqual(0);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `grid-toolbar-${label}`);

    // Filter: two conditions, OR, sent to the endpoint as q.
    await openTool(page, "Filter");
    await page.getByRole("button", { name: "Add condition", exact: true }).click();
    await page.getByRole("textbox", { name: "Value", exact: true }).fill("cafe");
    await page.getByRole("button", { name: "Add condition", exact: true }).click();
    await page.getByRole("combobox", { name: "Field" }).nth(1).selectOption("audience");
    await page.getByRole("combobox", { name: "Operator" }).nth(1).selectOption("gt");
    await page.getByRole("spinbutton", { name: "Value", exact: true }).fill("50");
    await page.getByRole("combobox", { name: "Join conditions" }).selectOption("or");
    await expect.poll(() => asked.at(-1)).toBe('name~="cafe"|audience>50');
    await shot(page, `grid-filter-${label}`);
    await page.keyboard.press("Escape");
    await expect(toolbar.locator("summary", { hasText: "Filter" })).toContainText("2");
    // The filters in force stay in sight as chips under the toolbar (T-6161).
    const chips = page.getByRole("list", { name: "Active filters" });
    await expect(chips.getByRole("listitem")).toHaveCount(3);
    await expect(chips).toContainText("contains cafe");
    await expect(chips.getByRole("listitem").nth(1)).toContainText(/^or\s*Places is greater than 50/);
    await shot(page, `grid-filter-chips-${label}`);

    // Search asks the endpoint across the text fields, beside the filter.
    await page.getByRole("searchbox", { name: "Search the rows" }).fill("Espoo");
    await expect.poll(() => asked.at(-1)).toContain('name~="(?i)Espoo"');
    await expect.poll(() => asked.at(-1)).toContain('address~="(?i)Espoo"');
    await page.getByRole("searchbox", { name: "Search the rows" }).fill("");

    // A footer count the endpoint makes over the whole filtered set.
    await page.getByRole("combobox", { name: "Footer of Places" }).selectOption("percentEmpty");
    await expect(page.getByText("25 %")).toBeVisible();
    await page.getByText("25 %").scrollIntoViewIfNeeded();
    await shot(page, `grid-footer-${label}`);

    // Select rows and delete them: one refused by the Policy stays, named with the reason.
    await page.getByRole("checkbox", { name: /^Select row: / }).nth(0).click();
    await page.getByRole("checkbox", { name: /^Select row: / }).nth(1).click();
    await expect(page.getByText("2 selected")).toBeVisible();
    await shot(page, `grid-selected-${label}`);
    await page.getByRole("button", { name: "Delete selected" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete selected" }).click();
    await expect(page.getByText("1 rows were not deleted")).toBeVisible();
    await expect(page.getByText(/the Policy does not grant this delete/)).toBeVisible();
    expect(deleted).toEqual(["urn:ngsi-ld:Event:hel.fi:linkedevents:1"]);
    // The refused row stays checked, so the bar keeps naming it.
    await expect(page.getByText("1 selected")).toBeVisible();
    await shot(page, `grid-delete-refused-${label}`);

    // Row height: four lines.
    await openTool(page, "Row height");
    await page.getByRole("radio", { name: "Large: four lines" }).check();
    await expect(page.locator(".jc-grid")).toHaveAttribute("data-row-size", "large");
    await page.keyboard.press("Escape");
    await shot(page, `grid-row-large-${label}`);

    // A chip's ✕ drops its condition and the read follows; Clear filters drops the rest (T-6161).
    await chips.getByRole("button", { name: /^Remove filter: .*contains cafe$/ }).click();
    await expect.poll(() => asked.includes("audience>50")).toBe(true);
    await expect(chips.getByRole("listitem")).toHaveCount(1);
    await chips.getByRole("button", { name: /^Remove filter: / }).click();
    await expect(page.getByRole("list", { name: "Active filters" })).toHaveCount(0);
    await shot(page, `grid-filter-cleared-${label}`);
  });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`The grid's bulk tools, live flash, colours and share (${label})`, async ({ page }) => {
    const seen: Recorded = { asked: [], deleted: [], patched: [], live: 0 };
    await page.setViewportSize({ width, height });
    await stubApi(page, seen.asked, seen.deleted, { linkml: LINKML_STATUS, seen });
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

    // A live change reads the rows again and flashes the one that changed, its reason named.
    await expect(page.getByText("Renamed while you looked")).toBeVisible();
    await expect(page.locator("tr.jc-grid-tr--info")).toHaveCount(1);
    await expect(page.locator("tr.jc-grid-tr--info")).toContainText("Renamed while you looked");
    await shot(page, `grid-live-flash-${label}`);

    // Colour by an enum field: one rule per value.
    await openTool(page, "Row colour");
    await page.getByRole("combobox", { name: "Colour by field" }).selectOption("status");
    await expect(page.getByRole("textbox", { name: "When" })).toHaveCount(2);
    await expect(page.getByRole("textbox", { name: "When" }).first()).toHaveValue('status=="open"');
    await shot(page, `grid-colour-by-${label}`);
    await page.keyboard.press("Escape");

    // Check three rows: export just them, then set a field in each through the Policy.
    for (const at of [0, 1, 3]) await page.getByRole("checkbox", { name: /^Select row: / }).nth(at).click();
    await expect(page.getByText("3 selected")).toBeVisible();
    const csv = page.getByRole("region", { name: "Selected rows" }).getByRole("link", { name: /CSV/i });
    await expect(csv).toHaveAttribute("href", /[?&]id=urn%3Angsi-ld%3AEvent%3Ahel\.fi%3Alinkedevents%3A1%2C/);
    await page.getByRole("button", { name: "Set a field" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("combobox", { name: "Field" }).selectOption("audience");
    await dialog.getByRole("textbox", { name: "New value" }).fill("4.5");
    await expect(dialog.getByText("This field takes a value of type integer.")).toBeVisible();
    await dialog.getByRole("combobox", { name: "Field" }).selectOption("status");
    await dialog.getByRole("combobox", { name: "New value" }).selectOption("cancelled");
    await expect(dialog.getByRole("button", { name: "Set in 3 rows" })).toBeDisabled();
    await dialog.getByRole("textbox", { name: "Why" }).fill("Venue closed for repairs");
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `grid-bulk-set-${label}`);
    await dialog.getByRole("button", { name: "Set in 3 rows" }).click();
    await expect(dialog.getByText("1 row was not changed")).toBeVisible();
    await expect(dialog.getByText(/the Policy does not grant status on this row/)).toBeVisible();
    expect(seen.patched.map((each) => each.id)).toEqual(["urn:ngsi-ld:Event:hel.fi:linkedevents:1", "urn:ngsi-ld:Event:hel.fi:linkedevents:4"]);
    expect(seen.patched[0]?.body).toEqual({ status: { type: "Property", value: "cancelled" } });
    await shot(page, `grid-bulk-set-refused-${label}`);
    await page.keyboard.press("Escape");

    // Share the view: the public Endpoint proposal opens from the toolbar with the view's fields.
    await openTool(page, "Share view");
    await expect(page.getByRole("textbox", { name: "Endpoint name" })).toBeVisible();
    await expect(page.getByText("the view's filter is not part of it")).toBeVisible();
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(sideways).toBeLessThanOrEqual(0);
    await shot(page, `grid-share-${label}`);
  });
}

// T-5188: the Sort panel's keys and the Group panel's fields reach the endpoint as one `orderBy`,
// each group gets a header with the endpoint's count, and the footer reads a maximum off the
// endpoint's own order. The space surface here filters, orders and counts like the broker does.
type Row = ReturnType<typeof event>;
const valueOf = (row: Row, attr: string): unknown => (row as Record<string, { value?: unknown } | undefined>)[attr]?.value;

function answer(url: URL): { rows: Row[]; total: number } {
  let rows = Array.from({ length: 24 }, (_, i) => event(i + 1));
  for (const term of (url.searchParams.get("q") ?? "").split(";").filter(Boolean)) {
    const equal = /^(\w+)=="(.*)"$/.exec(term);
    if (equal) rows = rows.filter((row) => valueOf(row, equal[1]) === equal[2]);
    else if (/^!\w+$/.test(term)) rows = rows.filter((row) => valueOf(row, term.slice(1)) === undefined);
    else if (/^\w+$/.test(term)) rows = rows.filter((row) => valueOf(row, term) !== undefined);
  }
  const keys = (url.searchParams.get("orderBy") ?? "").split(",").filter(Boolean).map((key) => key.split(";"));
  rows.sort((a, b) => {
    for (const [attr, dir] of keys) {
      const x = valueOf(a, attr);
      const y = valueOf(b, attr);
      if (x === y) continue;
      if (x === undefined || y === undefined) return x === undefined ? 1 : -1;
      const by = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y));
      return dir === "desc" ? -by : by;
    }
    return 0;
  });
  const offset = Number(url.searchParams.get("offset") ?? 0);
  const limit = Number(url.searchParams.get("limit") ?? 24);
  return { rows: rows.slice(offset, offset + limit), total: rows.length };
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`The grid sorts by three keys, groups with headers and reads a maximum (${label})`, async ({ page }) => {
    const orders: string[] = [];
    await page.setViewportSize({ width, height });
    await stubApi(page, [], []);
    await page.route((url) => url.pathname.startsWith("/cs/") && url.pathname.endsWith("/entities"), async (route) => {
      const url = new URL(route.request().url());
      if (route.request().method() !== "GET") return route.fallback();
      orders.push(url.searchParams.get("orderBy") ?? "");
      const { rows, total } = answer(url);
      return route.fulfill({ status: 200, contentType: "application/json", headers: { "NGSILD-Results-Count": String(total) }, body: JSON.stringify(rows) });
    });
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    const toolbar = page.getByTestId("grid-toolbar");
    await expect(page.getByText("Workshop for Families").first()).toBeVisible();

    // Sort: empty says what to do; two keys, the first turned round.
    await openTool(page, "Sort");
    await expect(page.getByText("No order set: the rows come as the endpoint keeps them.", { exact: false })).toBeVisible();
    await shot(page, `grid-sort-empty-${label}`);
    await page.getByRole("button", { name: "Add a sort key" }).click();
    await page.getByRole("combobox", { name: "Sort rows by, field 1" }).selectOption("startDate");
    await page.getByRole("combobox", { name: "Direction of sort key 1" }).selectOption("desc");
    await page.getByRole("button", { name: "Add a sort key" }).click();
    await page.getByRole("combobox", { name: "Sort rows by, field 2" }).selectOption("name");
    await expect.poll(() => orders.at(-1)).toBe("startDate;desc,name");
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `grid-sort-two-keys-${label}`);
    await page.keyboard.press("Escape");
    await expect(toolbar.locator("summary", { hasText: "Sort" })).toContainText("2");

    // Group by the start: the rows come ordered by it first, each date under a counted header.
    await openTool(page, "Group");
    await page.getByRole("button", { name: "Add a group field" }).click();
    await page.getByRole("combobox", { name: "Group rows by, field 1" }).selectOption("startDate");
    await expect.poll(() => orders.at(-1)).toBe("startDate;desc,name");
    await page.keyboard.press("Escape");
    const headers = page.locator("tr.jc-grid-group");
    await expect(headers.first()).toContainText("Starts");
    await expect(headers.first()).toContainText("2026-10-27");
    await expect(headers.first()).toContainText("3 rows");
    expect(await headers.count()).toBe(7);
    expect(await axeViolations(page)).toEqual([]);
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(sideways).toBeLessThanOrEqual(0);
    await shot(page, `grid-grouped-${label}`);

    // A second group level under the first.
    await openTool(page, "Group");
    await page.getByRole("button", { name: "Add a group field" }).click();
    await page.getByRole("combobox", { name: "Group rows by, field 2" }).selectOption("name");
    await page.keyboard.press("Escape");
    await expect(page.locator('tr.jc-grid-group[data-level="1"]').first()).toContainText("1 row");
    await shot(page, `grid-grouped-two-${label}`);

    // The footer's maximum is the first row of the endpoint's descending order among filled rows.
    await page.getByRole("combobox", { name: "Footer of Places" }).selectOption("max");
    await expect.poll(() => orders.includes("audience;desc")).toBe(true);
    await expect(page.locator("tfoot output").filter({ hasText: "230" })).toBeVisible();
    await page.locator("tfoot").scrollIntoViewIfNeeded();
    await shot(page, `grid-footer-max-${label}`);
  });
}

// T-5600: a name is a LanguageProperty on dev, and `name~=` compares its language map, which matched
// no row; a search that matched nothing then said no pipeline had written to the space.
for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`The search finds a name in any language and says when nothing matches (${label})`, async ({ page }) => {
    const asked: string[] = [];
    await page.setViewportSize({ width, height });
    await stubApi(page, asked, []);
    // Registered last, so it answers the rows: names in two languages, filtered by the term the grid sent.
    await page.route("**/cs/**/entities?*", async (route) => {
      const q = new URL(route.request().url()).searchParams.get("q") ?? "";
      asked.push(q);
      const rows = Array.from({ length: 24 }, (_, i) => event(i + 1)).map((row) => ({
        ...row,
        name: { type: "LanguageProperty", languageMap: { en: row.name.value, fi: `${row.name.value} (fi)` } },
      }));
      const term = /name\[\*\]~="\(\?i\)([^"]+)"/.exec(q)?.[1]?.toLowerCase();
      const found = term ? rows.filter((row) => Object.values(row.name.languageMap).some((name) => name.toLowerCase().includes(term))) : rows;
      return route.fulfill({ status: 200, contentType: "application/json", headers: { "NGSILD-Results-Count": String(found.length) }, body: JSON.stringify(found) });
    });
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    await expect(page.getByText("Workshop for Families").first()).toBeVisible();

    const search = page.getByRole("searchbox", { name: "Search the rows" });
    await search.fill("workshop");
    await expect.poll(() => asked.at(-1)).toContain('name[*]~="(?i)workshop"');
    expect(asked.at(-1)).not.toContain("audience~=");
    expect(asked.at(-1)).not.toContain("startDate~=");
    await expect(page.getByText("Board game club")).toHaveCount(0);
    await expect(page.getByText("Workshop for Families").first()).toBeVisible();
    await page.getByText("Workshop for Families").first().scrollIntoViewIfNeeded();
    await shot(page, `grid-search-language-${label}`);

    await search.fill("Herttoniemi");
    const none = page.getByRole("status").filter({ hasText: "No row of Event matches “Herttoniemi”." });
    await expect(none).toBeVisible();
    await expect(page.getByText("no pipeline has written")).toHaveCount(0);
    // The empty answer keeps the columns' kinds: the next ask still leaves the number and date out.
    expect(asked.filter((q) => q.includes("Herttoniemi")).every((q) => !q.includes("audience~=") && !q.includes("startDate~="))).toBe(true);
    expect(await axeViolations(page)).toEqual([]);
    await none.scrollIntoViewIfNeeded();
    await shot(page, `grid-search-no-match-${label}`);

    await none.getByRole("button", { name: "Clear the search" }).click();
    await expect(search).toHaveValue("");
    await expect(page.getByText("Board game club").first()).toBeVisible();
  });
}

// T-5596: on a touch screen every control of the space page is a finger's 44 px, the grid's
// checkboxes through their labels, the crumbs, brand, back and CSV links through `touch-target`;
// a tap beside a row's box ticks it and leaves the row closed. Nothing scrolls sideways.
test.describe("The space page at phone width with touch", () => {
  test.use({ hasTouch: true, isMobile: true });
  for (const [width, height] of [[400, 860], [360, 740]] as const) {
    test(`44 px targets at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await stubApi(page, [], []);
      await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
      await expect(page.getByText("Workshop for Families").first()).toBeVisible();
      expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches), "the phone is a touch screen").toBe(true);

      expect(await smallTouchTargets(page), "every control is at least 44 x 44 px").toEqual([]);
      // The filter chips under the toolbar keep the finger's size too (T-6161).
      await openTool(page, "Filter");
      await page.getByRole("button", { name: "Add condition", exact: true }).click();
      await page.getByRole("textbox", { name: "Value", exact: true }).fill("cafe");
      await page.getByRole("button", { name: "Add condition", exact: true }).click();
      // A finger closes the phone's Tools menu by its Close button (T-6463).
      await page.getByTestId("grid-toolbar").getByRole("button", { name: "Close" }).tap();
      await expect(page.getByRole("button", { name: "Add condition", exact: true })).toBeHidden();
      await expect(page.getByRole("list", { name: "Active filters" }).getByRole("listitem")).toHaveCount(3);
      expect(await smallTouchTargets(page), "every chip control is at least 44 x 44 px").toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth), "nothing scrolls sideways").toBeLessThanOrEqual(width);
      // The box keeps its size: the label around it is what grew.
      const box = await page.getByRole("checkbox", { name: /^Select row: / }).first().boundingBox();
      expect(box?.width).toBeLessThan(24);
      // The frozen name column starts where the row-number column ends, not over it.
      const rownum = await page.locator("tbody td.jc-grid-rownum").first().boundingBox();
      const name = await page.locator("tbody td.jc-grid-pinned").first().boundingBox();
      expect(name!.x).toBeGreaterThanOrEqual(rownum!.x + rownum!.width - 0.5);

      // A tap on the label beside the box ticks the row and opens nothing.
      const target = page.locator("tbody .jc-grid-check").first();
      await target.scrollIntoViewIfNeeded();
      const area = await target.boundingBox();
      await page.touchscreen.tap(area!.x + 3, area!.y + 3);
      await expect(page.getByRole("checkbox", { name: /^Select row: / }).first()).toBeChecked();
      await expect(page.locator("dialog[open]")).toHaveCount(0);
      await shot(page, `grid-touch-${width}`);
    });
  }
});

// T-5601: a busy space surface (429 with Retry-After) is a retry that waits as asked and then
// loads the rows; only a refusal (404) lists the space's endpoints.
for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`A busy space surface is a retry, a refusal names the endpoints (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, [], []);
    let answer = 429;
    await page.route(`**/cs/${SPACE}/**/entities?*`, async (route) => {
      if (answer === 200) return route.fallback();
      const headers: Record<string, string> = answer === 429 ? { "Retry-After": "2" } : {};
      return route.fulfill({ status: answer, contentType: "application/json", headers, body: JSON.stringify({ title: answer === 429 ? "Too Many Requests" : "Not Found" }) });
    });
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

    const alert = page.getByRole("alert").filter({ hasText: "The space's data did not answer" });
    await expect(alert).toContainText("The Portal is busy right now; nothing was changed. Try again in 2 seconds.");
    const retry = alert.getByRole("button", { name: "Retry" });
    await expect(retry).toHaveAttribute("aria-disabled", "true");
    await expect(page.getByText("You can see this space through its endpoints:")).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `probe-busy-${label}`);

    await expect(retry).not.toHaveAttribute("aria-disabled", "true", { timeout: 5000 });
    answer = 200;
    await retry.click();
    await expect(page.getByText("Workshop for Families").first()).toBeVisible();
    await shot(page, `probe-retried-${label}`);

    answer = 404;
    await page.reload();
    await expect(page.getByText("You can see this space through its endpoints:")).toBeVisible();
    await expect(page.getByRole("link", { name: "app-helsinki-data" })).toBeVisible();
    await expect(page.getByRole("alert").filter({ hasText: "did not answer" })).toHaveCount(0);
    await shot(page, `probe-refused-${label}`);
  });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`The bulk bar says when checked rows are not loaded (F-UI3-014, ${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, [], []);
    // A search that leaves the first row out: it stays checked, but no longer loaded.
    await page.route(
      (url) => url.pathname.endsWith("/entities") && (url.searchParams.get("q") ?? "").includes("Espoo"),
      (route) =>
        route.fulfill({
          status: 200,
          contentType: "application/json",
          headers: { "NGSILD-Results-Count": "23" },
          body: JSON.stringify(Array.from({ length: 23 }, (_, i) => event(i + 2))),
        }),
    );
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    await expect(page.getByText("Workshop for Families").first()).toBeVisible();
    await page.getByRole("checkbox", { name: /^Select row: / }).nth(0).click();
    await page.getByRole("checkbox", { name: /^Select row: / }).nth(1).click();
    await expect(page.getByText("2 selected")).toBeVisible();
    await expect(page.getByText(/are loaded: Delete selected/)).toHaveCount(0);

    await page.getByRole("searchbox", { name: "Search the rows" }).fill("Espoo");
    await expect(page.getByText("1 of 2 are loaded: Delete selected and Set a field act on those 1.", { exact: false })).toBeVisible();
    await shot(page, `bulk-not-loaded-${label}`);
  });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`An address with a broken escape keeps the shell and its breadcrumb (F-UI1-002, ${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, [], []);
    // vite preview answers a malformed escape 404 where the Portal serves the app shell: serve it here.
    await page.route(
      (url) => url.pathname.includes("%E0%A4%A"),
      async (route) => route.fulfill({ response: await route.fetch({ url: new URL("/", route.request().url()).href }) }),
    );
    await page.goto(`/projects/${PROJECT}/spaces/%E0%A4%A?lang=en`);
    await expect(page.getByRole("main")).toBeVisible();
    await expect(page.getByRole("banner")).toBeVisible();
    await shot(page, `broken-escape-${label}`);
  });
}

