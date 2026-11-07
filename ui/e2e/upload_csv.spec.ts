/**
 * Uploading a CSV into a space, in a browser (T-3840; DM-81).
 *
 * `tests/upload_csv.test.tsx` holds the page's logic under jsdom. This run is the page at its own
 * address: a file given to the picker is read in the browser, an unreachable catalogue still
 * leaves the file its own model, and nothing is proposed before a space is named.
 */
import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";
import { zip } from "../tests/xlsxFixture";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "banskabystrica";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@bb.sk",
  roles: ["portal-editor"],
};

async function stub(page: import("@playwright/test").Page, writes: string[]) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (request.method() !== "GET") writes.push(url.pathname);
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
    if (url.pathname === "/api/v1/tools/sdm-catalog") return json({ title: "unreachable" }, 503);
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

test("a CSV is read in the browser and waits for a type and a space before anything is proposed", async ({ page }) => {
  const writes: string[] = [];
  await stub(page, writes);

  await page.goto(`/projects/${PROJECT}/spaces/upload?lang=en`);
  await expect(page.getByRole("heading", { level: 1, name: "Upload a CSV" })).toBeVisible();
  await page.getByLabel("Choose a CSV file").setInputFiles({
    name: "garages.csv",
    mimeType: "text/csv",
    buffer: Buffer.from("name,capacity\nNorth,120\nSouth,80\n"),
  });

  await expect(page.getByText(/None of the Smart Data Models types fits these columns/)).toBeVisible();
  // No type fits: no one-option radio, the type name alone (T-6567).
  await expect(page.getByRole("radio")).toHaveCount(0);
  // The sheet names the space and the type (T-6567); without a type name Propose holds back with its reason.
  await expect(page.getByRole("combobox", { name: "Context Space", exact: true })).toHaveValue("garages");
  const typeName = page.getByLabel("Type name");
  await expect(typeName).toHaveValue("Garage");
  await typeName.fill("");
  const propose = page.getByRole("button", { name: "Propose the change" });
  await expect(propose).toHaveAttribute("aria-disabled", "true");
  await expect(propose).toHaveAccessibleDescription(/Choose a type, or write your own type name/);
  expect(await axeViolations(page)).toEqual([]);
  expect(writes, "nothing is written before the change is proposed").toEqual([]);
});

for (const width of [1440, 400]) {
  test(`an Excel workbook is read as its first sheet at ${width} px (JC1-07)`, async ({ page }) => {
    const writes: string[] = [];
    await stub(page, writes);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`/projects/${PROJECT}/spaces/upload?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: "Upload a CSV" })).toBeVisible();
    const cell = (ref: string, text: string) => `<c r="${ref}" t="inlineStr"><is><t>${text}</t></is></c>`;
    const row = (n: number, values: string[]) =>
      `<row r="${n}">${values.map((v, i) => cell(`${"AB"[i]}${n}`, v)).join("")}</row>`;
    const sheet = `<worksheet><sheetData>${row(1, ["name", "capacity"])}${row(2, ["North", "120"])}${row(3, ["South", "80"])}</sheetData></worksheet>`;
    await page.getByLabel("Choose a CSV file").setInputFiles({
      name: "garages.xlsx",
      mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
      buffer: Buffer.from(zip({ "xl/worksheets/sheet1.xml": sheet })),
    });
    await expect(page.getByText(/None of the Smart Data Models types fits these columns/)).toBeVisible();
    await expect(page.getByText("garages.xlsx: 2 rows, 2 columns.")).toBeVisible();
    // No type fits: no one-option radio, the type name alone (T-6567).
    await expect(page.getByRole("radio")).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    expect(writes, "nothing is written before the change is proposed").toEqual([]);
    await page.screenshot({ path: `test-results/T-6372/upload-xlsx-${width}.png`, fullPage: true });
  });
}
