import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// A pipeline's rejected records grouped by rule and path (G-21, T-7069) as a steward meets them:
// the largest group first with where its fix belongs, an enum's allowed values, and the CSV
// export. `vite preview` has no Portal API behind it, so the API is answered in the browser.

const IDENTITY = { subject: "s1", username: "demo.steward", name: "Demo Steward", email: "steward@bbsk.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "ovzdusie-stanice", namespace: "bbsk" },
  spec: { class: "scheduled" },
  status: { phase: "Live", conditions: [{ type: "Scheduled", status: "True" }] },
};
const record = (id: number, rule: string, path: string, message: string, value: unknown) => ({
  id,
  at: `2026-11-15T0${id % 10}:00:00Z`,
  record: { id: `urn:ngsi-ld:AirQualityObserved:bbsk:ovzdusie:s-${id}`, type: "AirQualityObserved", [path]: { type: "Property", value } },
  rule,
  path,
  message,
});
const datatype = (id: number) =>
  record(id, "sh:datatype", "pm10", "the attribute pm10 breaks the data model of AirQualityObserved: its value must be a number", "n/a");
const enumBroken = (id: number) =>
  record(id, "sh:in", "window", "the attribute window breaks the data model of AirQualityObserved: its value is not one of the values the model lists", "week");
const GROUPS = {
  total: 2140,
  groups: [
    { rule: "sh:datatype", path: "pm10", count: 1874, fix: "mapping", examples: [datatype(9), datatype(8), datatype(7)] },
    { rule: "sh:in", path: "window", count: 212, fix: "mapping", allowed: ["day", "month"], examples: [enumBroken(6)] },
    { rule: "sh:closed", path: "stationColour", count: 41, fix: "model", examples: [record(5, "sh:closed", "stationColour", "the attribute stationColour breaks the data model of AirQualityObserved: the model does not list it on a closed class", "red")] },
    { rule: "step", path: "", count: 13, fix: "step", examples: [record(4, "step", "", "step 3 threw: date 31.09.2026 is not a calendar day", "x")] },
  ],
};

async function stubApi(page: Page, groups: unknown, exportStatus = 200): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "bbsk", grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read", "propose", "approve"] } }] });
    }
    if (path.endsWith("/rejected/groups")) return json(groups);
    if (path.endsWith("/rejected/export")) {
      if (exportStatus !== 200) {
        return json({ type: "about:blank", title: "Service Unavailable", status: exportStatus,
          detail: "the data model air of project bbsk could not be read, so its personal attributes cannot be withheld; try again in a minute" }, exportStatus);
      }
      return route.fulfill({ status: 200, contentType: "text/csv", body: "id,at,rule,path,step,run,message,record\r\n" });
    }
    if (path.endsWith("/rejected")) {
      const items = (groups as typeof GROUPS).groups.flatMap((g) => g.examples);
      return json({ items, total: (groups as typeof GROUPS).total });
    }
    if (path.endsWith("/pipelines")) return json(list([PIPELINE]));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  const width = page.viewportSize()?.width ?? 1440;
  await page.setViewportSize({ width, height: Math.max(height, 860) });
  await page.screenshot({ path: join(dir, `${name}.png`) });
}

async function openRejected(page: Page): Promise<void> {
  await page.goto("/projects/bbsk/pipelines?lang=en");
  await page.getByRole("button", { name: "More actions for ovzdusie-stanice" }).click();
  await page.getByRole("menuitem", { name: "Rejected records" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
}

test.describe("rejected records grouped by rule (G-21)", () => {
  for (const width of [1440, 400]) {
    test(`a steward sees what to fix first and exports the records at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, GROUPS);
      await openRejected(page);
      const section = page.getByRole("region", { name: "By rule" });
      const cards = section.getByRole("listitem");
      await expect(cards).toHaveCount(4);
      await expect(cards.nth(0)).toContainText("1,874 records");
      await expect(cards.nth(0)).toContainText("Fix in the mapping");
      await cards.nth(1).getByText("sh:in").click();
      await expect(cards.nth(1)).toContainText("day, month");
      await cards.nth(1).getByText("sh:in").click();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `rejected-groups-filled-${width}`);
      const download = page.waitForEvent("download");
      await section.getByRole("button", { name: "Export CSV" }).click();
      expect((await download).suggestedFilename()).toBe("bbsk-ovzdusie-stanice-rejected.csv");
    });

    test(`an export the Portal cannot write says why at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, GROUPS, 503);
      await openRejected(page);
      const section = page.getByRole("region", { name: "By rule" });
      await section.getByRole("button", { name: "Export CSV" }).click();
      await expect(section.getByRole("alert")).toContainText("personal attributes cannot be withheld");
      await shot(page, `rejected-groups-error-${width}`);
    });

    test(`a pipeline with nothing rejected shows no groups at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, { total: 0, groups: [] });
      await openRejected(page);
      await expect(page.getByRole("dialog")).toContainText("No rejected records");
      await expect(page.getByRole("region", { name: "By rule" })).toHaveCount(0);
      await shot(page, `rejected-groups-empty-${width}`);
    });
  }
});
