/**
 * T-5326, T-4667: a data steward's hand on one value, on dev. On an entity of the journey's own
 * (a `t5326-` AirQualityObserved of the helsinki space, created and removed here) the steward edits
 * the current value, adds an old one, corrects it, reverts it and imports a CSV (preview counts,
 * then confirm), each in the row's Correct values tab, whose timeline names the steward and the
 * reason. An old value leaves the current one alone (DM-106). A viewer is not offered the tab and
 * the gateway refuses the same write. Needs ANTARES_HISTORY_BACKFILL=on on the broker and the
 * seeded Policy steward-history-journey, the steward's only write of this type.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, VIEWER, csrf, signIn } from "./portal";

const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const URN = `urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:t5326-${SUFFIX}`;
const SPACE = "/cs/helsinki/ngsi-ld/v1";
const ROW = `/projects/helsinki/spaces/helsinki?lang=en&type=AirQualityObserved&row=${encodeURIComponent(URN)}`;
const NOW = new Date(Date.now() - 3_600_000).toISOString();
/** A `datetime-local` value of two days ago and the same instant as the CSV writes it. */
const OLD = new Date(Date.now() - 2 * 86_400_000);
const OLD_LOCAL = new Date(OLD.getTime() - OLD.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
const CSV_AT = new Date(Date.now() - 3 * 86_400_000).toISOString().slice(0, 16) + ":00Z";

async function currentValue(page: Page): Promise<unknown> {
  const answer = await page.request.get(`${SPACE}/entities/${encodeURIComponent(URN)}`);
  expect(answer.status()).toBe(200);
  return ((await answer.json()) as { pm10: { value: unknown } }).pm10.value;
}

async function fill(region: ReturnType<Page["getByRole"]>, value: string, reason: string): Promise<void> {
  await region.getByRole("textbox", { name: /Value/ }).fill(value);
  await region.getByRole("textbox", { name: /Reason/ }).fill(reason);
}

test("a steward's every hand on a value stands in the timeline, and a viewer has none", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/spaces/helsinki?lang=en");
  let viewer: { context: BrowserContext; page: Page } | undefined;
  try {
    // As the UI writes it: plain JSON, so the view names the attributes by the space's @context.
    const headers = { "x-csrf-token": await csrf(context), "JC-Reason": "t5326 journey", "Content-Type": "application/json" };
    const created = await page.request.post(`${SPACE}/entities`, {
      headers,
      data: {
        id: URN,
        type: "AirQualityObserved",
        pm10: { type: "Property", value: 20, observedAt: NOW },
      },
    });
    expect(created.status(), await created.text()).toBe(201);

    await page.goto(ROW);
    const dialog = page.getByRole("dialog").first();
    await dialog.getByRole("tab", { name: "Correct values" }).click();
    await dialog.getByRole("combobox", { name: "Attribute" }).selectOption("pm10");
    const timeline = dialog.getByRole("list", { name: "Timeline" });
    await expect(timeline).toBeVisible();

    await dialog.getByRole("region", { name: "Edit current value" }).getByText("Edit current value").click();
    await fill(dialog.getByRole("region", { name: "Edit current value" }), "21", "t5326 recalibrated");
    await dialog.getByRole("region", { name: "Edit current value" }).getByRole("button", { name: "Save" }).click();
    await expect(timeline.getByRole("listitem").filter({ hasText: "t5326 recalibrated" })).toContainText("demo.steward");
    expect(await currentValue(page)).toBe(21);

    const old = dialog.getByRole("region", { name: "Add an old value" });
    await old.getByText("Add an old value").click();
    await old.getByLabel(/Observed at/).fill(OLD_LOCAL);
    await fill(old, "9", "t5326 paper log");
    await old.getByRole("button", { name: "Save" }).click();
    await expect(timeline.getByText(/t5326 paper log/)).toBeVisible();
    expect(await currentValue(page), "an old value leaves the current one alone").toBe(21);

    await timeline.getByRole("listitem").filter({ hasText: "t5326 paper log" }).getByRole("button", { name: "Correct" }).click();
    const correcting = dialog.getByRole("region", { name: /Correct measurement/ });
    await fill(correcting, "8", "t5326 misread");
    await correcting.getByRole("button", { name: "Save" }).click();
    await expect(timeline.getByText(/t5326 misread/)).toBeVisible();

    await timeline.getByRole("listitem").filter({ hasText: "t5326 paper log" }).getByRole("button", { name: "Revert" }).click();
    const reverting = dialog.getByRole("region", { name: /Revert to older version/ });
    await reverting.getByRole("textbox", { name: /Reason/ }).fill("t5326 first reading right");
    await reverting.getByRole("button", { name: "Revert" }).click();
    await expect(timeline.getByText(/t5326 first reading right/)).toBeVisible();
    expect(await currentValue(page), "a past correction leaves the current one alone").toBe(21);

    const csv = dialog.getByRole("region", { name: "Import old values from CSV" });
    await csv.getByText("Import old values from CSV").click();
    await csv.getByLabel(/CSV file/).setInputFiles({ name: "old.csv", mimeType: "text/csv", buffer: Buffer.from(`observedAt,value\n${CSV_AT},7\n`) });
    await expect(csv.getByRole("status")).toContainText("New: 1 · Duplicates: 0 · Corrections: 0");
    await csv.getByRole("textbox", { name: /Reason/ }).fill("t5326 sheet two");
    await csv.getByRole("button", { name: "Confirm import" }).click();
    await expect(timeline.getByText(/t5326 sheet two/)).toBeVisible();

    viewer = await signIn(browser, VIEWER, ROW);
    const seen = viewer.page.getByRole("dialog").first();
    await expect(seen.getByRole("tab", { name: "History" })).toBeVisible();
    await expect(seen.getByRole("tab", { name: "Correct values" })).toHaveCount(0);
    const refused = await viewer.page.request.patch(`${SPACE}/entities/${encodeURIComponent(URN)}/attrs`, {
      headers: { "x-csrf-token": await csrf(viewer.context), "JC-Reason": "t5326 not mine", "Content-Type": "application/json" },
      data: { pm10: { type: "Property", value: 1, observedAt: new Date().toISOString() } },
    });
    expect(refused.status(), "the gateway refuses a viewer's write").toBe(403);
    expect(await currentValue(page)).toBe(21);
  } finally {
    await viewer?.context.close();
    const headers = { "x-csrf-token": await csrf(context), "JC-Reason": "t5326 journey leaves nothing" };
    await page.request.delete(`${SPACE}/entities/${encodeURIComponent(URN)}`, { headers });
    await context.close();
  }
});
