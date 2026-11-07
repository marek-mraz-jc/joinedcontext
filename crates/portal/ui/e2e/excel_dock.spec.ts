/**
 * The table's whole way in, from the assistant's dock (T-6456): attach a workbook, read and correct
 * the drafted model, propose, approve in place, load the rows, all in the dock and at both widths.
 *
 * Run with SHOTS_DIR set to keep the 1440 and 400 pictures of each state.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { workbook } from "../tests/xlsxFixture";

const PROJECT = "helsinki";
const API = "joinedcontext.com/v1alpha1";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: [], groups: [] };
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const LINKML = `id: https://example.org/models/Garage
name: Garage
imports: [ngsi-ld-core]
classes:
  Garage:
    is_a: Entity
    slots: [name, spots, zip]
slots:
  name: { range: string, required: true, title: { en: name } }
  spots: { range: integer, title: { en: spots } }
  zip: { range: integer, title: { en: zip } }
`;
const BOOK = {
  name: "garages.xlsx",
  mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  buffer: Buffer.from(workbook({ Garages: [["name", "spots", "zip"], ["Kamppi", 500, "00100"], ["Kluuvi", 420, "00120"]] })),
};
const complete = (space: string, source: string, change: unknown) => ({
  space,
  found: [],
  drafts: [{ kind: "DataModel", name: space, inferred: true, manifest: { kind: "DataModel", metadata: { name: space }, spec: { classes: ["Garage"], source } }, verdict: { ok: true, findings: [], inputDigest: "a" } }],
  proposeReady: true,
  lane: "red",
  change,
});

interface World {
  phase: string;
  ops: Record<string, unknown>[];
  written: unknown[];
}

async function stub(page: Page, world: World): Promise<void> {
  const change = () => ({
    apiVersion: API,
    kind: "Change",
    metadata: { name: "chg-9", namespace: PROJECT },
    status: { phase: world.phase, lane: "red", plan: { create: 1, update: 0, delete: 0 } },
    summary: { key: "change.summary.create", params: { kind: "ContextSpace", name: "garages" } },
    author: { name: "Aino", email: "aino@hel.fi" },
  });
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/ops/jc_space_complete")) {
      const body = request.postDataJSON() as Record<string, unknown> & { files: { name: string; content: string }[] };
      world.ops.push(body);
      const given = body.files.find((f) => f.name.endsWith(".linkml.yaml"))?.content;
      return json(complete("garages", given ?? LINKML, body.propose === true ? change() : null));
    }
    if (path.endsWith("/changes/chg-9/approve")) {
      world.phase = "Merged";
      return json(change());
    }
    if (path.endsWith("/changes/chg-9")) return json(change());
    if (path.endsWith("/policies/garages-upload")) return json({ kind: "Policy", metadata: { name: "garages-upload" }, spec: {}, status: { phase: "Live" } });
    if (path.endsWith("/spaces/garages")) return json({ apiVersion: API, kind: "ContextSpace", metadata: { name: "garages", namespace: PROJECT }, spec: {} });
    if (path.endsWith("/endpoints")) {
      return json(list([{ kind: "Endpoint", metadata: { name: "garages-all" }, spec: { contextSpaceRef: { name: "garages" }, slug: "garagesslugaaaaaaaaaaaaaaaaaa", exposes: [] }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/entityOperations/upsert")) {
      world.written.push(...(request.postDataJSON() as unknown[]));
      return route.fulfill({ status: 204 });
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: PROJECT }]), kind: "ProjectList" });
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (/^\/api\/v1\/projects\/[a-z-]+\/[a-z-]+$/.test(path)) return json(list([]));
    return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
  });
}

const shot = async (page: Page, name: string) => {
  if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/${name}.png` });
};

for (const [label, width, height] of [["1440", 1440, 900], ["400", 400, 860]] as const) {
  test.describe(`at ${label} px`, () => {
    test.use({ viewport: { width, height } });

    test("attach, correct the model, propose, approve and load, all in the dock", async ({ page }) => {
      const world: World = { phase: "PendingApproval", ops: [], written: [] };
      await stub(page, world);
      await page.goto(`/projects/${PROJECT}?lang=en`);
      await page.getByRole("button", { name: "Open the assistant" }).click();
      await page.getByLabel(/^Attach a table/).setInputFiles(BOOK);

      const dock = page.getByTestId("assistant-table");
      await expect(dock.getByText("garages.xlsx: 2 rows, 3 columns.")).toBeVisible();
      await expect(dock.getByLabel("Files", { exact: true })).toHaveCount(0);
      await expect(page).not.toHaveURL(/spaces\/complete/);
      await shot(page, `dock-attached-${label}`);

      await dock.getByRole("button", { name: "Check my file", exact: true }).click();
      const review = dock.getByTestId("model-review");
      await expect(review).toContainText("Kamppi");
      await review.getByLabel("zip").selectOption("string");
      await shot(page, `dock-review-${label}`);
      expect(await axeViolations(page)).toEqual([]);

      await dock.getByRole("button", { name: "Propose all" }).click();
      const approve = dock.getByRole("button", { name: "Approve", exact: true });
      await expect(approve).toBeVisible();
      expect(world.ops.at(-1)?.files).toEqual(expect.arrayContaining([expect.objectContaining({ name: "garages.linkml.yaml", content: expect.stringContaining("range: string") })]));
      await shot(page, `dock-proposed-${label}`);

      await dock.getByRole("textbox", { name: "Type the name to confirm" }).fill("garages");
      await approve.click();
      const write = dock.getByRole("button", { name: "Write 2 rows" });
      await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 15_000 });
      await shot(page, `dock-live-${label}`);

      await write.click();
      await expect(dock.getByRole("status").filter({ hasText: "2 rows written, none refused." })).toBeVisible();
      await shot(page, `dock-loaded-${label}`);
      expect(world.written).toHaveLength(2);
      expect(world.written[0]).toMatchObject({ zip: { value: "00100" } });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    });
  });
}
