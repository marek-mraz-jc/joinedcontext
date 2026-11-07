import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import { axeViolations } from "./axe";

// The row's Correct values tab (T-5326): a person who may write an attribute edits the current
// value, adds an old one, corrects a measurement, reverts it and imports a CSV, each landing in
// the timeline with who and why; a person who only reads is not offered the tab. `vite preview`
// has no Portal API behind it, so the browser plays the broker: a write of an instant it holds is
// a new version, of a new instant a new measurement. With SHOTS_DIR set, the 1440 px and 400 px
// screenshots go there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki-kpi";
const TYPE = "KeyPerformanceIndicator";
const URN = `urn:ngsi-ld:${TYPE}:hel.fi:kpi:3`;
const SLUG = "kpiendpointkpiendpointkpiend";
const PERSON = "demo.steward@hel.fi";

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
  "    slots: [id, name, kpiValue]",
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name }",
  "  kpiValue: { range: float, title: Value }",
  "",
].join("\n");

function kpi(n: number) {
  return {
    id: `urn:ngsi-ld:${TYPE}:hel.fi:kpi:${n}`,
    type: TYPE,
    name: { type: "Property", value: `District ${n}` },
    kpiValue: { type: "Property", value: 10 * n, observedAt: "2026-10-20T10:00:00Z" },
  };
}

interface Row {
  type: "Property";
  value: unknown;
  observedAt: string;
  instanceId: string;
  supersededAt?: string;
  provenance: Record<string, string>;
}

const written = (value: unknown, observedAt: string, n: number, provenance: Record<string, string>): Row => ({
  type: "Property",
  value,
  observedAt,
  instanceId: `urn:ngsi-ld:Instance:${n}`,
  provenance,
});

/** What the broker keeps of the attribute, oldest write first, and what a PATCH does to it. */
function broker() {
  const rows = [written(30, "2026-10-20T10:00:00.000Z", 0, { channel: "pipeline", operation: "create", actor: "serviceAccount:pl-kpi-import", writtenAt: "2026-10-20T10:00:05Z" })];
  return {
    rows,
    write(attribute: { value: unknown; observedAt: string }, reason: string) {
      const at = new Date(attribute.observedAt).toISOString();
      const held = rows.findLast((row) => row.observedAt === at && !row.supersededAt);
      const now = new Date().toISOString();
      if (held) held.supersededAt = now;
      rows.push(written(attribute.value, at, rows.length, { channel: "ui", operation: held ? "correct" : "update", actor: `user:${PERSON}`, reason, writtenAt: now }));
    },
  };
}

async function stubApi(page: Page, store: ReturnType<typeof broker>, writes: string[], refuse = false): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = decodeURIComponent(url.pathname);
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "b7c1e0f4", username: "demo.steward", name: "Demo Steward", roles: ["portal-editor"] });
    if (path.endsWith("/datamodels/helsinki-kpi/source")) return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    if (path.endsWith("/access/attributes")) return json(attributeTable(TYPE, ["name", "kpiValue"], ["kpiValue"]));
    if (path.includes("/temporal/entities/")) {
      const before = url.searchParams.get("timeAt");
      if (store.rows.length === 0) return json({ title: "Not Found" }, 404);
      return json({ id: URN, type: TYPE, kpiValue: store.rows.filter((row) => !before || row.observedAt < before) });
    }
    if (request.method() === "PATCH" && path.endsWith(`/entities/${URN}/attrs`)) {
      writes.push(path);
      if (refuse) return json({ title: "Forbidden", detail: "the Policy of this Endpoint grants no write of kpiValue" }, 403);
      const reason = decodeURIComponent(request.headers()["jc-reason"] ?? "");
      if (!reason) return json({ title: "Reason Required" }, 400);
      store.write((request.postDataJSON() as { kpiValue: { value: unknown; observedAt: string } }).kpiValue, reason);
      return route.fulfill({ status: 204 });
    }
    if (path.endsWith(`/entities/${URN}`)) return json(kpi(3));
    if (path.endsWith("/entities")) {
      const rows = Array.from({ length: 12 }, (_, i) => kpi(i + 1));
      return json(rows, 200, { "NGSILD-Results-Count": String(rows.length) });
    }
    if (path.endsWith(`/spaces/${SPACE}/comments`)) return json([]);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/usage`)) return json({ entities: 12, broker: "main", observedAt: "2026-10-20T16:00:00Z" });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki-kpi" }));
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki-kpi", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml: LINKML, artifacts: { jsonSchema: "kpi.schema.json" } })]));
    }
    if (path.endsWith("/endpoints")) return json(list([manifest("Endpoint", "kpi", { contextSpaceRef: SPACE, slug: SLUG, types: [TYPE] })]));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (!dir) return;
  // The whole tab, not the fold: the dialog scrolls, so the frame is made as tall as the page wants.
  const { width } = page.viewportSize() ?? { width: 1440 };
  await page.setViewportSize({ width, height: 1400 });
  await page.screenshot({ path: `${dir}/${name}.png` });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`a steward edits, backfills, corrects, reverts and imports, each with who and why (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const store = broker();
    const writes: string[] = [];
    await stubApi(page, store, writes);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await dialog.getByRole("tab", { name: "Correct values" }).click();
    const timeline = dialog.getByRole("list", { name: "Timeline" });
    await expect(timeline.getByText(/pl-kpi-import/)).toBeVisible();
    await shot(page, `history-loaded-${label}`);

    // Nothing leaves without a reason.
    const current = dialog.getByRole("region", { name: "Edit current value" });
    await current.getByText("Edit current value").click();
    await current.getByRole("textbox", { name: /Value/ }).fill("many");
    await expect(current.getByText("Enter a value of this field's type")).toBeVisible();
    await shot(page, `history-invalid-${label}`);
    await current.getByRole("textbox", { name: /Value/ }).fill("35");
    await current.getByRole("textbox", { name: /Reason/ }).fill("Recalibrated on site");
    await current.getByRole("button", { name: "Save" }).click();
    await expect(timeline.getByText(/Recalibrated on site/)).toBeVisible();
    await expect(dialog.getByText("Saved. The change stands in the timeline.")).toBeVisible();
    await expect(timeline.getByText(new RegExp(PERSON)).first()).toBeVisible();

    const old = dialog.getByRole("region", { name: "Add an old value" });
    await old.getByText("Add an old value").click();
    await old.getByLabel(/Observed at/).fill("2026-03-03T14:00");
    await old.getByRole("textbox", { name: /Value/ }).fill("9");
    await old.getByRole("textbox", { name: /Reason/ }).fill("From the paper log");
    await old.getByRole("button", { name: "Save" }).click();
    await expect(timeline.getByText(/From the paper log/)).toBeVisible();

    const row = timeline.getByRole("listitem").filter({ hasText: "From the paper log" });
    await row.getByRole("button", { name: "Correct" }).click();
    const correcting = dialog.getByRole("region", { name: /Correct measurement/ });
    await correcting.getByRole("textbox", { name: /Value/ }).fill("8");
    await correcting.getByRole("textbox", { name: /Reason/ }).fill("Misread digit");
    await shot(page, `history-correcting-${label}`);
    await correcting.getByRole("button", { name: "Save" }).click();
    await expect(timeline.getByText(/Misread digit/)).toBeVisible();

    await timeline.getByRole("listitem").filter({ hasText: "From the paper log" }).getByRole("button", { name: "Revert" }).click();
    const reverting = dialog.getByRole("region", { name: /Revert to older version/ });
    await expect(reverting.getByText("Writes 9 again")).toBeVisible();
    await reverting.getByRole("textbox", { name: /Reason/ }).fill("The first reading was right");
    await reverting.getByRole("button", { name: "Revert" }).click();
    await expect(timeline.getByText(/The first reading was right/)).toBeVisible();
    await shot(page, `history-versions-${label}`);

    const csv = dialog.getByRole("region", { name: "Import old values from CSV" });
    await csv.getByText("Import old values from CSV").click();
    await csv.getByLabel(/CSV file/).setInputFiles({
      name: "old.csv",
      mimeType: "text/csv",
      buffer: Buffer.from("observedAt,value\n2026-10-20T10:00:00Z,30\n2026-10-20T09:00:00Z,28\n2026-10-20T08:00:00Z,oops\n"),
    });
    await expect(csv.getByText("New: 1 · Duplicates: 1 · Corrections: 0")).toBeVisible();
    await expect(csv.getByText(/Line 4/)).toBeVisible();
    await shot(page, `history-csv-preview-${label}`);
    const before = writes.length;
    await csv.getByRole("textbox", { name: /Reason/ }).fill("Paper log, sheet 2");
    await csv.getByRole("button", { name: "Confirm import" }).click();
    await expect(timeline.getByText(/Paper log, sheet 2/)).toBeVisible();
    expect(writes.length, "the duplicate and the broken row stay home").toBe(before + 1);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `history-imported-${label}`);
  });

  test(`nothing recorded says so, and a refused write says why (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const store = broker();
    store.rows.length = 0;
    await stubApi(page, store, [], true);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await dialog.getByRole("tab", { name: "Correct values" }).click();
    await expect(dialog.getByText("No history yet")).toBeVisible();
    await shot(page, `history-empty-${label}`);
    const current = dialog.getByRole("region", { name: "Edit current value" });
    await current.getByText("Edit current value").click();
    await current.getByRole("textbox", { name: /Value/ }).fill("35");
    await current.getByRole("textbox", { name: /Reason/ }).fill("Recalibrated on site");
    await current.getByRole("button", { name: "Save" }).click();
    await expect(current.getByText(/grants no write of kpiValue/)).toBeVisible();
    await shot(page, `history-refused-${label}`);
  });

  test(`a person with no write is not offered the tab (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, broker(), []);
    await page.route("**/access/attributes**", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify(attributeTable(TYPE, ["name", "kpiValue"])) }),
    );
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await expect(dialog.getByRole("tab", { name: "History" })).toBeVisible();
    await expect(dialog.getByRole("tab", { name: "Correct values" })).toHaveCount(0);
    await shot(page, `history-viewer-${label}`);
  });
}
