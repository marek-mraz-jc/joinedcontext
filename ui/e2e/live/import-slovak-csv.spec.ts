/**
 * A person loads a Slovak CSV into a space on dev without help (T-3248, ADR-N-042 §3.6).
 *
 * The file is what a Slovak Excel saves: Windows-1250, semicolons, a decimal comma and dates as
 * day.month.year. The steward opens the helsinki space's AirQualityObserved data, chooses Import
 * rows, sees how the file was read and its first rows, finds every column mapped by its name, and
 * creates the rows; the entities answer with the numbers and dates converted. A `finally` deletes
 * them through the gateway, so dev keeps nothing of the journey.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const LOCAL = [`t3248-${SUFFIX}-a`, `t3248-${SUFFIX}-b`];
const URN = (local: string) => `urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:${local}`;

/** The CSV as Windows-1250 bytes: ASCII as itself, `á` as 0xE1, the one other letter it holds. */
function windows1250(text: string): Buffer {
  return Buffer.from([...text].map((char) => (char === "á" ? 0xe1 : char.charCodeAt(0))));
}

test("a Slovak CSV with decimal commas and day.month.year dates is loaded into a space", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/spaces/helsinki?lang=en");
  try {
    await page.locator("#space-inside-type").selectOption("AirQualityObserved");
    await page.getByRole("button", { name: "Import rows" }).click();
    const dialog = page.getByRole("dialog", { name: /Import rows into AirQualityObserved/ });
    await dialog.getByLabel("Choose a file").setInputFiles({
      name: "ovzdusie.csv",
      mimeType: "text/csv",
      buffer: windows1250(
        `id;pm10;dateObserved;poznámka\n${LOCAL[0]};12,5;7.10.2026 8:00;Banská Bystrica\n${LOCAL[1]};1 204,25;7. 10. 2026 9:30;\n`,
      ),
    });

    await expect(dialog.getByTestId("import-detected")).toHaveText(
      "Encoding WINDOWS-1250 · separated by semicolons · decimal comma · dates as d.m.yyyy",
    );
    await dialog.getByText(/^Preview the first 2 rows$/).click();
    await expect(dialog.getByRole("table", { name: "The file's first rows as read" })).toContainText("1 204,25");
    await expect(dialog.getByRole("combobox", { name: "pm10" })).toHaveValue("pm10");
    await expect(dialog.getByRole("combobox", { name: "dateObserved" })).toHaveValue("dateObserved");
    await expect(dialog.getByRole("table", { name: "The file's first rows as read" })).toContainText("Banská Bystrica");
    await expect(dialog.getByRole("status").first()).toContainText("2 rows are ready");

    await dialog.getByRole("button", { name: "Create 2 rows" }).click();
    await expect(dialog.getByText("2 rows created, none refused.")).toBeVisible({ timeout: 60_000 });

    const read = await page.request.get(
      `/cs/helsinki/ngsi-ld/v1/entities/${encodeURIComponent(URN(LOCAL[1]))}?options=keyValues`,
    );
    expect(read.ok(), "the created row answers").toBe(true);
    const entity = (await read.json()) as { pm10?: number; dateObserved?: string };
    expect(entity.pm10).toBe(1204.25);
    expect(entity.dateObserved?.startsWith("2026-10-07")).toBe(true);
  } finally {
    const headers = { "x-csrf-token": await csrf(context) };
    for (const local of LOCAL) {
      await page.request.delete(`/cs/helsinki/ngsi-ld/v1/entities/${encodeURIComponent(URN(local))}`, { headers });
    }
    await context.close();
  }
});
