import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The Metadata tab of a row's attribute (T-5529): created and changed from sysAttrs, and the last
// writer's kind, organization, channel and reason from one `versions=all&lastN=1` read. The editor
// reads versions; the viewer's versions read is a 403 and every provenance field says why it is
// hidden. `vite preview` has no Portal API behind it, so the browser answers it. With SHOTS_DIR set,
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
  "    slots: [id, name, kpiValue, category]",
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name }",
  "  kpiValue: { range: float, title: Value }",
  "  category: { range: string, title: Category }",
  "",
].join("\n");

function kpi(n: number) {
  return {
    id: `urn:ngsi-ld:${TYPE}:hel.fi:kpi:${n}`,
    type: TYPE,
    name: { type: "Property", value: `District ${n}` },
    kpiValue: { type: "Property", value: 10 * n, observedAt: "2026-10-20T10:00:00Z", createdAt: "2026-10-01T07:30:00Z", modifiedAt: "2026-10-20T11:02:00Z" },
    category: { type: "Property", value: "quantitative", createdAt: "2026-10-01T07:30:00Z", modifiedAt: "2026-10-01T07:30:00Z" },
  };
}

const VERSIONS: Record<string, unknown[]> = {
  kpiValue: [
    {
      type: "Property", value: 30, observedAt: "2026-10-20T10:00:00Z",
      provenance: {
        channel: "ui", operation: "correct", actor: "jana.kovacova", actorKind: "person", organization: "helsinki",
        reason: "Typo in the import", writtenAt: "2026-10-20T11:02:00Z",
      },
    },
  ],
  category: [
    {
      type: "Property", value: "quantitative", observedAt: "2026-10-01T07:30:00Z",
      provenance: { channel: "pipeline", operation: "create", actor: "pl-kpi-import", actorKind: "pipeline", organization: "helsinki", writtenAt: "2026-10-01T07:30:00Z" },
    },
  ],
};

async function stubApi(page: Page, readVersions: boolean): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [readVersions ? "portal-editor" : "portal-viewer"] });
    }
    if (path.endsWith("/datamodels/helsinki-kpi/source")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    }
    if (path.includes("/temporal/entities/")) {
      asked.push(url.search);
      if (url.searchParams.get("versions") === "all" && !readVersions) {
        return json({ type: "https://uri.etsi.org/ngsi-ld/errors/Forbidden", title: "Forbidden", status: 403 }, 403);
      }
      const attr = url.searchParams.get("attrs") ?? "";
      return json({ id: URN, type: TYPE, [attr]: VERSIONS[attr] ?? [] });
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
      return json(list([manifest("DataModel", "helsinki-kpi", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml: LINKML })]));
    }
    return json(list([]));
  });
  return asked;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

function field(dialog: ReturnType<Page["getByRole"]>, term: string) {
  return dialog.locator(".jc-grid-meta .jc-grid-detail-item").filter({ has: dialog.page().getByText(term, { exact: true }) }).locator("dd");
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`the editor reads who changed an attribute, a person or a pipeline (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const asked = await stubApi(page, true);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await dialog.getByRole("tab", { name: "Metadata" }).click();
    await dialog.getByRole("combobox", { name: "Attribute" }).selectOption("kpiValue");
    await expect(field(dialog, "Organization")).toHaveText("helsinki");
    await expect(field(dialog, "Changed by")).toHaveText("jana.kovacova (a person)");
    await expect(field(dialog, "Through")).toHaveText("Portal");
    await expect(field(dialog, "Reason")).toHaveText("Typo in the import");
    await expect(field(dialog, "Last changed").locator("time")).toHaveAttribute("datetime", "2026-10-20T11:02:00Z");
    expect(asked.at(-1)).toContain("versions=all");
    expect(asked.at(-1)).toContain("lastN=1");
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `meta-editor-person-${label}`);

    await dialog.getByRole("combobox", { name: "Attribute" }).selectOption("category");
    await expect(field(dialog, "Changed by")).toHaveText("pl-kpi-import (a pipeline)");
    await expect(field(dialog, "Through")).toHaveText("Pipeline");
    await expect(field(dialog, "Reason")).toHaveText("No reason was given");
    await shot(page, `meta-editor-pipeline-${label}`);
  });

  test(`the viewer without readVersions sees the times and why the rest is hidden (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const asked = await stubApi(page, false);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en&row=${encodeURIComponent(URN)}`);
    const dialog = page.getByRole("dialog", { name: "Details: District 3" });
    await dialog.getByRole("tab", { name: "Metadata" }).click();
    await dialog.getByRole("combobox", { name: "Attribute" }).selectOption("kpiValue");
    // One refusal covers who, where from, through what and why: said once, on "Changed by".
    await expect(field(dialog, "Changed by")).toHaveText("hidden: no access (history not granted)");
    for (const term of ["Organization", "Through", "Reason"]) {
      await expect(field(dialog, term)).toHaveCount(0);
    }
    await expect(field(dialog, "Created").locator("time")).toHaveAttribute("datetime", "2026-10-01T07:30:00Z");
    // A 403 is shown, never asked again under another identity: one read per attribute picked.
    expect(asked.filter((query) => query.includes("attrs=kpiValue&versions=all"))).toHaveLength(1);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `meta-viewer-hidden-${label}`);
  });
}
