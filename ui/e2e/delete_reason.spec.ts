import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// DM-113 (T-5341): the explorer's delete asks why, and the gateway gets the person's words
// percent-encoded as JC-Reason. `vite preview` has no API and no gateway, so both are answered
// in the browser.

const TYPE = "BikeHireDockingStation";
const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const ID = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:001`;
const ROW = { id: ID, type: TYPE, name: { type: "Property", value: "Dock 1" }, availableBikeNumber: { type: "Property", value: 7, observedAt: "2026-10-24T08:00:00Z" } };
/** Three readings of the bike count, for the History tab. */
const READINGS = [5, 6, 7].map((value, i) => ({ type: "Property", value, observedAt: `2026-10-24T0${6 + i}:00:00Z` }));
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = ["id: https://hel.fi/models/mobility", "name: helsinki-mobility", "classes:", `  ${TYPE}:`, "    slots: [id, name, availableBikeNumber]", "slots:", "  id: {}", "  name: { range: string }", "  availableBikeNumber: { range: integer }", ""].join("\n");

async function stub(page: Page, mayDelete = true): Promise<(string | null)[]> {
  const reasons: (string | null)[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (request.method() === "DELETE") {
      reasons.push(request.headers()["jc-reason"] ?? null);
      return route.fulfill({ status: 204, body: "" });
    }
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "jana", name: "Jana Kováčová", roles: [] });
    if (path.endsWith("/access/check")) return json({ decision: mayDelete || !(request.postData() ?? "").includes("deleteEntity") });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(request.postData() ?? "{}").evaluations as { action: { name: string } }[]).map((item) => ({ decision: mayDelete || item.action.name !== "deleteEntity" })) });
    if (path.endsWith("/access/attributes")) return json(attributeTable(TYPE, ["name"]));
    if (path.endsWith("/access")) {
      const read = { actions: ["queryEntity", "retrieveEntity", "queryTemporal", "retrieveTemporal"], resource: { type: TYPE }, attributes: "*" };
      return json({ permissions: mayDelete ? [read, { actions: ["deleteEntity"], resource: { type: TYPE } }] : [read], prohibitions: [] });
    }
    if (path.includes("/temporal/entities/")) {
      const attr = new URL(request.url()).searchParams.get("attrs") ?? "";
      return json({ id: ID, type: TYPE, [attr]: attr === "availableBikeNumber" ? READINGS : [] });
    }
    if (path.endsWith(`/entities/${encodeURIComponent(ID)}`)) return json(ROW);
    if (path.endsWith("/entities")) return json([ROW]);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility" } }, spec: { dataModelRef: "helsinki-mobility" }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "helsinki-bikes", namespace: "helsinki", title: { en: "Helsinki city bike stations" } }, spec: { contextSpaceRef: "mobility", slug: SLUG, audience: "organization", enabledRepresentations: ["ngsi-ld"] }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "helsinki-mobility", namespace: "helsinki" }, spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML } }]));
    }
    return json(list([]));
  });
  return reasons;
}

const SHOTS = process.env.T5341_SHOTS ?? "test-results/delete-reason";

for (const width of [1440, 400]) {
  test(`a delete asks why and sends the reason, at ${width} px`, async ({ page }) => {
    mkdirSync(SHOTS, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    const reasons = await stub(page);
    await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    await page.getByRole("button", { name: "Open: Dock 1" }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByTestId("explore-delete").click();
    const reason = dialog.getByLabel(/^Reason/);
    await expect(reason).toBeVisible();
    await page.screenshot({ path: `${SHOTS}/delete-reason-empty-${width}.png` });

    await page.getByTestId("explore-delete-confirm").click();
    await expect(dialog.getByText("Enter why you delete it, for example: Duplicate entry.")).toBeVisible();
    expect(reasons).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/delete-reason-error-${width}.png` });

    await reason.fill("Duplikát, ďalší je 12");
    await page.screenshot({ path: `${SHOTS}/delete-reason-filled-${width}.png` });
    await page.getByTestId("explore-delete-confirm").click();
    await expect(dialog).toBeHidden();
    expect(reasons).toEqual([encodeURIComponent("Duplikát, ďalší je 12")]);
  });
}

// T-5619: a row of the explorer opens in the grid's own detail, its fields drawn by type, a History
// tab, the removal as the page's action and the JSON-LD folded away; Escape gives the grid back.
for (const width of [1440, 400]) {
  test(`a row opens its detail, its history and a refused delete, at ${width} px`, async ({ page }) => {
    mkdirSync(SHOTS, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    const reasons = await stub(page, false);
    await page.goto(`/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&lang=en`);
    const open = page.getByRole("button", { name: "Open: Dock 1" }).first();
    await open.focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog", { name: "Details: Dock 1" });
    await expect(dialog.getByRole("tab", { name: "Fields" })).toHaveAttribute("aria-selected", "true");
    await expect(dialog).not.toContainText("@context");
    await expect(dialog).not.toContainText(ID);
    await expect(dialog.getByTestId("explore-technical")).not.toHaveAttribute("open");
    // Refused with its reason, reachable by keyboard (UI-44).
    await expect(dialog.getByTestId("explore-delete")).toHaveAttribute("aria-disabled", "true");
    await expect(dialog.getByText("This endpoint's grant does not let you delete a BikeHireDockingStation").first()).toBeAttached();
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/row-detail-${width}.png` });
    await dialog.getByTestId("explore-delete").scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${SHOTS}/row-delete-refused-${width}.png` });

    await dialog.getByRole("tab", { name: "History" }).click();
    await dialog.getByLabel("Attribute").selectOption("availableBikeNumber");
    await expect(dialog.getByText("6").first()).toBeVisible();
    await page.screenshot({ path: `${SHOTS}/row-history-${width}.png` });

    await dialog.getByRole("tab", { name: "Fields" }).click();
    await dialog.getByText("Technical details (JSON-LD)").click();
    await expect(dialog.getByRole("group", { name: "Entity" })).toContainText(ID);
    await page.screenshot({ path: `${SHOTS}/row-technical-${width}.png` });

    await dialog.getByRole("tab", { name: "Fields" }).focus();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    // The focus is back on the grid, its active cell the one that opened the row.
    await expect(page.getByRole("grid")).toBeFocused();
    expect(reasons).toEqual([]);
  });
}
