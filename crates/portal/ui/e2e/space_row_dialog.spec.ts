import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// A space row as a dialog (T-4828): `?row=<urn>` opens it, Fields, History with every version and
// who changed it, Comments as tabs; "Add row" opens the type's form. `vite preview` has no Portal
// API behind it, so the API and the space surface are answered in the browser. With SHOTS_DIR set,
// the 1440 px and 400 px screenshots are written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki-kpi";
const TYPE = "KeyPerformanceIndicator";
const URN = `urn:ngsi-ld:${TYPE}:hel.fi:kpi:3`;

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
  status: { phase: "Live" },
});

const LINKML = [
  "id: https://hel.fi/models/kpi",
  "name: helsinki-kpi",
  "classes:",
  "  KeyPerformanceIndicator:",
  "    title: Key performance indicator",
  "    slots: [id, name, kpiValue, category, address]",
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name }",
  "  kpiValue: { range: float, title: Value }",
  "  category: { range: KpiCategory, title: Category }",
  "  address: { range: string, title: Address }",
  "enums:",
  "  KpiCategory:",
  "    permissible_values:",
  "      quantitative: { title: Quantitative }",
  "      qualitative: { title: Qualitative }",
  "",
].join("\n");

function kpi(n: number) {
  return {
    id: `urn:ngsi-ld:${TYPE}:hel.fi:kpi:${n}`,
    type: TYPE,
    name: { type: "Property", value: `District ${n}` },
    kpiValue: { type: "Property", value: 10 * n, observedAt: "2026-10-20T10:00:00Z" },
    category: { type: "Property", value: "quantitative" },
    // An attribute the import left empty: the detail says "not set" (T-5599).
    address: { type: "Property", value: {} },
  };
}

const VERSIONS = {
  id: URN,
  type: TYPE,
  kpiValue: [
    {
      type: "Property", value: 29, observedAt: "2026-10-20T10:00:00Z", supersededAt: "2026-10-20T11:02:00Z",
      provenance: { channel: "pipeline", operation: "create", actor: "kpi-import", writtenAt: "2026-10-20T10:00:04Z" },
    },
    {
      type: "Property", value: 30, observedAt: "2026-10-20T10:00:00Z",
      provenance: { channel: "ui", operation: "correct", actor: "jana.kovacova", reason: "Typo in the import", writtenAt: "2026-10-20T11:02:00Z" },
    },
    { type: "Property", value: 27, observedAt: "2026-10-19T10:00:00Z", provenance: { channel: "pipeline", operation: "create", actor: "kpi-import" } },
  ],
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path.endsWith("/datamodels/helsinki-kpi/source")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    }
    if (path.includes("/temporal/entities/")) {
      const all = url.searchParams.get("versions") === "all";
      return json(all ? VERSIONS : { ...VERSIONS, kpiValue: VERSIONS.kpiValue.filter((one) => !one.supersededAt) });
    }
    if (path.endsWith(`/entities/${URN}`)) return json(kpi(3));
    if (path.endsWith("/entities")) {
      const rows = Array.from({ length: 12 }, (_, i) => kpi(i + 1));
      return json(rows, 200, { "NGSILD-Results-Count": String(rows.length) });
    }
    if (path.endsWith(`/spaces/${SPACE}/comments`)) {
      return json([{ id: "c1", urn: URN, author: "jana.kovacova", text: "Checked against the city report.", createdAt: "2026-10-20T12:00:00Z", mine: true }]);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/usage`)) return json({ entities: 12, broker: "main", observedAt: "2026-10-20T16:00:00Z" });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki-kpi" }));
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki-kpi", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml: LINKML })]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`a row opens as a dialog from its link, with history versions and comments (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);

    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("tab")).toHaveText(["Fields", "History", "Metadata", "Changes", "Comments"]);
    await expect(dialog.getByText("30", { exact: true })).toBeVisible();
    // T-5599: no URN on screen (Copy identifier holds it), the enum's label, and "not set".
    await expect(dialog).not.toContainText("urn:");
    await expect(dialog.getByRole("button", { name: "Copy identifier" })).toBeVisible();
    await expect(dialog.getByText("Quantitative", { exact: true })).toBeVisible();
    await expect(dialog.getByText("not set", { exact: true })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `row-fields-${label}`);

    await dialog.getByRole("tab", { name: "History" }).click();
    await dialog.getByRole("combobox", { name: "Attribute" }).selectOption("kpiValue");
    await dialog.getByRole("checkbox", { name: /Every version/ }).check();
    await expect(dialog.getByRole("cell", { name: "Typo in the import" })).toBeVisible();
    await expect(dialog.getByRole("cell", { name: "pipeline · create · kpi-import" }).first()).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `row-history-versions-${label}`);

    await dialog.getByRole("tab", { name: "Comments" }).click();
    await expect(dialog.getByText("Checked against the city report.")).toBeVisible();
    await shot(page, `row-comments-${label}`);

    // Escape closes it and the address no longer names the row.
    await dialog.getByRole("tab", { name: "Comments" }).focus();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(new URL(page.url()).searchParams.get("row")).toBeNull();

    await page.getByRole("button", { name: "Add row" }).click();
    const form = page.getByRole("dialog", { name: /^New / });
    await expect(form.getByRole("button", { name: "Create", exact: true })).toBeVisible();
    // DM-113 (T-5341): the form asks why, starting from a sentence the person may change.
    const reason = form.getByRole("textbox", { name: /^Reason/ });
    await expect(reason).toHaveValue("Entered through the form");
    await reason.clear();
    await form.getByRole("button", { name: "Create", exact: true }).click();
    await expect(form.getByText("Enter why you add it, for example: Inspection on 2026-10-22.")).toBeVisible();
    await shot(page, `row-add-${label}`);
  });
}

// The History tab's answers (T-5597): a 404 for an attribute with nothing recorded says so with no
// retry, a 403 names the missing right with no retry, a 503 offers a retry that reads again.
for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`the History tab tells nothing recorded, a refusal and a failure apart (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    let failed = false;
    await page.route("**/cs/**/temporal/entities/**", async (route) => {
      const asked = new URL(route.request().url()).searchParams;
      // The Changes tab's own read (T-6333) is answered by stubApi; this route judges History.
      if (asked.get("versions") === "all" && asked.get("attrs")?.includes(",")) return route.fallback();
      const attrs = asked.get("attrs");
      const answer = (status: number, body: unknown) =>
        route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
      if (attrs === "name") return answer(404, { type: "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound", title: "Not Found" });
      if (attrs === "category") return answer(403, { type: "https://uri.etsi.org/ngsi-ld/errors/PermissionDenied", title: "Forbidden" });
      if (!failed) {
        failed = true;
        return answer(503, { title: "Service Unavailable", detail: "the broker did not answer" });
      }
      return route.fallback();
    });
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await dialog.getByRole("tab", { name: "History" }).click();
    const attribute = dialog.getByRole("combobox", { name: "Attribute" });

    // History opens on the measured value (T-5602): its first read is the one that fails.
    await expect(attribute).toHaveValue("kpiValue");
    await expect(dialog.getByRole("alert")).toContainText("Not loaded: the broker did not answer");
    await shot(page, `row-history-failed-${label}`);
    await dialog.getByRole("button", { name: "Try again" }).click();
    await expect(dialog.getByRole("cell", { name: "30" }).first()).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await shot(page, `row-history-values-${label}`);

    await attribute.selectOption("name");
    await expect(dialog.getByText("Nothing was recorded in this window.")).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await shot(page, `row-history-nothing-${label}`);

    await attribute.selectOption("category");
    await expect(dialog.getByRole("alert")).toContainText("Your role may read current values but not their history");
    await expect(dialog.getByRole("button", { name: "Try again" })).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `row-history-refused-${label}`);
  });
}

// T-5602: Back after closing a row leaves the space at once, and a link names the row's History
// tab and attribute.
for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`back after a closed row leaves the space, a link opens its history (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces?lang=en`);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
    // The search is written in place: it adds no step to Back.
    await page.getByRole("searchbox", { name: "Search the rows" }).fill("District");
    await expect.poll(() => new URL(page.url()).searchParams.get("q")).toBe("District");
    await page.getByRole("button", { name: "Open: District 3" }).click();
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await dialog.getByRole("tab", { name: "History" }).click();
    // History opens on the measured value, not on the name.
    await expect(dialog.getByRole("combobox", { name: "Attribute" })).toHaveValue("kpiValue");
    await expect.poll(() => new URL(page.url()).searchParams.get("tab")).toBe("history");
    // The close goes back to the entry before the open: the rows stay, nothing is read again.
    const reads: string[] = [];
    page.on("request", (request) => {
      if (/\/entities(\?|$)/.test(new URL(request.url()).pathname + "?")) reads.push(request.url());
    });
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(new URL(page.url()).searchParams.get("row")).toBeNull();
    expect(new URL(page.url()).searchParams.get("q")).toBe("District");
    await page.waitForTimeout(500);
    expect(reads).toEqual([]);

    const started = Date.now();
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/spaces\\?lang=en$`));
    const took = Date.now() - started;
    console.log(`T-5602 back from a closed row (${label}): ${took} ms`);
    expect(took).toBeLessThan(1000);

    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}&tab=history&attr=kpiValue&q=District`);
    const linked = page.getByRole("dialog", { name: "Details: District 3" });
    await expect(linked.getByRole("tab", { name: "History" })).toHaveAttribute("aria-selected", "true");
    await expect(linked.getByRole("combobox", { name: "Attribute" })).toHaveValue("kpiValue");
    await shot(page, `row-history-link-${label}`);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("searchbox", { name: "Search the rows" })).toHaveValue("District");
    await shot(page, `row-search-kept-${label}`);
  });
}

// T-6333: the Changes tab, from the versions the broker keeps. A rename kept by `JC-Record: version`
// and a corrected reading show newest first with who, organization, channel and reason; a field's
// "changed" mark opens the tab on it; a 403 (no readVersions) hides tab and marks; a 404 says nothing
// is kept yet; a 503 offers Try again. The plain grid read never asks for versions.
const RENAMES = {
  id: URN,
  type: TYPE,
  name: [
    { type: "Property", value: "District three", instanceId: "n1", provenance: { channel: "pipeline", operation: "create", actor: "serviceAccount:kpi-import", actorKind: "pipeline", organization: "hel", writtenAt: "2026-10-01T08:00:00Z", audit: true } },
    { type: "Property", value: "District 3", instanceId: "n2", provenance: { channel: "ui", operation: "update", actor: "user:jana.kovacova", actorKind: "person", organization: "hel", reason: "Name as in the city register", writtenAt: "2026-10-21T09:15:00Z", audit: true } },
  ],
  kpiValue: VERSIONS.kpiValue.map((one) => ({ ...one, provenance: { ...one.provenance, organization: "hel", actorKind: one.provenance.channel === "ui" ? "person" : "pipeline" } })),
};

for (const lang of ["en", "sk"] as const) {
  for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
    test(`the Changes tab: filled, empty, refused and failed (${lang}, ${label})`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await stubApi(page);
      let mode: "filled" | "empty" | "refused" | "failed" = "filled";
      const plainReads: string[] = [];
      await page.route("**/cs/**/entities**", async (route) => {
        const url = new URL(route.request().url());
        if (!url.pathname.includes("/temporal/")) {
          plainReads.push(url.search);
          return route.fallback();
        }
        if (url.searchParams.get("versions") !== "all") return route.fallback();
        const answer = (status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
        if (mode === "refused") return answer(403, { type: "https://uri.etsi.org/ngsi-ld/errors/PermissionDenied", title: "Forbidden", detail: "readVersions is not granted" });
        if (mode === "empty") return answer(404, { type: "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound", title: "Not Found" });
        if (mode === "failed") return answer(503, { title: "Service Unavailable" });
        return answer(200, RENAMES);
      });
      const t = lang === "en"
        ? { tab: "Changes", details: "Details", show: "Show the changes of Name", attr: "Attribute", person: "Changed by", all: "All", empty: "No change of this row is kept yet", error: "The changes could not be read.", retry: "Try again" }
        : { tab: "Zmeny", details: "Podrobnosti", show: "Zobraziť zmeny atribútu Name", attr: "Atribút", person: "Zmenil(a)", all: "Všetko", empty: "Zatiaľ nie je uchovaná žiadna zmena", error: "Zmeny sa nepodarilo načítať.", retry: "Skúsiť znova" };
      const open = async () => {
        await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=${lang}&row=${encodeURIComponent(URN)}`);
        const dialog = page.getByRole("dialog").first();
        await expect(dialog).toBeVisible();
        return dialog;
      };

      let dialog = await open();
      const mark = dialog.getByRole("button", { name: t.show });
      await expect(mark).toBeVisible();
      await shot(page, `changes-mark-${lang}-${label}`);
      await mark.click();
      await expect(dialog.getByRole("tab", { name: t.tab })).toHaveAttribute("aria-selected", "true");
      await expect(dialog.getByRole("combobox", { name: t.attr })).toHaveValue("name");
      await expect(dialog.getByRole("listitem")).toHaveCount(1);
      await expect(dialog.getByRole("listitem").first()).toContainText("Name as in the city register");
      await expect(dialog.getByRole("listitem").first()).toContainText("District three");
      await dialog.getByRole("combobox", { name: t.attr }).selectOption("");
      await expect(dialog.getByRole("listitem")).toHaveCount(2);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `changes-filled-${lang}-${label}`);
      // The correction's writer changed no name: both filters together match nothing.
      await dialog.getByRole("combobox", { name: t.attr }).selectOption("name");
      await dialog.getByRole("combobox", { name: t.person }).selectOption({ value: "jana.kovacova" });
      await expect(dialog.getByRole("listitem")).toHaveCount(0);
      await shot(page, `changes-filtered-empty-${lang}-${label}`);
      await dialog.getByRole("button", { name: lang === "en" ? "Show every change" : "Zobraziť všetky zmeny" }).click();
      await expect(dialog.getByRole("listitem")).toHaveCount(2);
      // The grid's own reads never ask for versions; only the Changes read does.
      expect(plainReads.filter((query) => query.includes("versions"))).toEqual([]);

      mode = "empty";
      dialog = await open();
      await dialog.getByRole("tab", { name: t.tab }).click();
      await expect(dialog.getByText(new RegExp(t.empty))).toBeVisible();
      await expect(dialog.getByRole("button", { name: t.show })).toHaveCount(0);
      await shot(page, `changes-empty-${lang}-${label}`);

      mode = "failed";
      dialog = await open();
      await dialog.getByRole("tab", { name: t.tab }).click();
      await expect(dialog.getByRole("alert")).toContainText(t.error);
      await shot(page, `changes-error-${lang}-${label}`);
      mode = "filled";
      await dialog.getByRole("button", { name: t.retry }).click();
      await expect(dialog.getByRole("listitem")).toHaveCount(2);

      mode = "refused";
      const refused = page.waitForResponse((response) => response.status() === 403 && response.url().includes("versions=all"));
      dialog = await open();
      await refused;
      await expect(dialog.getByRole("tab", { name: t.tab })).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: t.show })).toHaveCount(0);
      await expect(dialog).not.toContainText("readVersions");
      await shot(page, `changes-refused-${lang}-${label}`);
    });
  }
}

// T-6344: the grid as the owner left it (no row numbers, filter row, Type picker in its filter cell)
// and a cell click opening that attribute's view: its value, metadata and history in one dialog.
for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`a cell opens its attribute's view with value, metadata and history (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

    const grid = page.getByRole("grid").first();
    await expect(grid.getByText("District 3", { exact: true })).toBeVisible();
    await shot(page, `grid-rows-${label}`);

    // A phone has no filter row (T-6463): the header's funnel filters there too.
    await expect(grid.getByRole("row", { name: "Filters" })).toHaveCount(width < 640 ? 0 : 1);
    await grid.getByRole("button", { name: "Filter column: Value" }).click();
    await shot(page, `grid-filter-row-${label}`);
    // The popover lies above the frozen checkbox column: its title is what a press there reaches.
    const popover = page.locator(".jc-grid-autofilter");
    const box = await popover.boundingBox();
    expect(box).not.toBeNull();
    // A gutter from the screen edges, as the page keeps.
    expect(box!.x).toBeGreaterThanOrEqual(8);
    expect(box!.x + box!.width).toBeLessThanOrEqual(width - 8);
    expect(await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest(".jc-grid-autofilter") !== null, [box!.x + 12, box!.y + 14])).toBe(true);
    await page.keyboard.press("Escape");

    const row = grid.getByRole("row").filter({ hasText: "District 3" });
    await row.getByRole("gridcell", { name: "30", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await expect(dialog).toBeVisible();
    await expect(dialog.locator(".jc-grid-attr-value")).toHaveText("30");
    expect(await axeViolations(page)).toEqual([]);
    // The versions table scrolls inside its card: the dialog itself never runs off a phone.
    expect(await dialog.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
    await shot(page, `attribute-view-number-${label}`);

    await dialog.locator(".jc-grid-detail-attr select").selectOption("category");
    await expect(dialog.locator(".jc-grid-attr-value")).toHaveText("Quantitative");
    await shot(page, `attribute-view-state-${label}`);
  });
}
