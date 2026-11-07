/**
 * The table's way in from the assistant (T-6456): a person with only a project editor's rights
 * attaches one .xlsx in the dock, reads the drafted model there, proposes it, and after another
 * person's approval loads every row in the same dock. The same file attached again opens nothing
 * new. Counted through the space's Endpoint. Left out of the sweep: run by hand after a pin.
 *
 *   EXCEL_VIDEO=<dir> keeps the video and SHOTS_DIR the pictures of each state;
 *   LIVE_WIDTH=400 runs it at phone width.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, EDITOR, STEWARD, approve, proposedChange, rejectOpen, removeCompletely, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const WORKBOOK = join(dirname(fileURLToPath(import.meta.url)), "fixtures/kulturne-miesta-bbsk.xlsx");
const SHEET = "Kultúrne miesta";
const TYPE = "KulturneMiesta";
const ROWS = 338;
const SPACE = `t6456-excel-${Date.now().toString().slice(-6)}`;
const SHOTS = process.env.SHOTS_DIR;
const VIDEO = process.env.EXCEL_VIDEO;
const WIDTH = Number(process.env.LIVE_WIDTH ?? "1440");
const SIZE = { width: WIDTH, height: WIDTH < 500 ? 860 : 900 };

const pace = async (page: Page, ms = 1_500) => {
  if (VIDEO) await page.waitForTimeout(ms);
};
async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${state}-${WIDTH}.png`) });
}
async function attach(page: Page): Promise<void> {
  await page.getByLabel(/^Attach a table/).setInputFiles(WORKBOOK);
  const dock = page.getByTestId("assistant-table");
  await dock.locator("#complete-sheet-0").selectOption(SHEET);
  await expect(dock.locator("#complete-sheet-0")).toContainText(`${ROWS} rows`);
  await dock.locator("#complete-space").fill(SPACE);
}

test.describe.configure({ mode: "serial" });
test.setTimeout(600_000);

let editor: { context: BrowserContext; page: Page };

test.afterAll(async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}?lang=en`);
  await rejectOpen(steward, PROJECT, SPACE, "excel_assistant");
  const removed = async (plural: string, name: string) => removeCompletely(steward, PROJECT, plural, name).catch((refused: unknown) => console.log(String(refused)));
  for (const plural of ["endpoints", "policies"]) {
    const answer = await steward.page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
    const items = answer.ok() ? (((await answer.json()) as { items?: { metadata: { name: string } }[] }).items ?? []) : [];
    for (const { metadata } of items.filter((one) => one.metadata.name.startsWith(SPACE))) await removed(plural, metadata.name);
  }
  await removed("spaces", SPACE);
  await sweepDrafts(editor.context, editor.page, PROJECT, new RegExp(`^${SPACE}`));
  await steward.context.close();
  const video = editor.page.video();
  await editor.context.close();
  if (VIDEO) await video?.saveAs(join(VIDEO, "assistant-flow.webm"));
});

test("one .xlsx in the assistant: model reviewed, approved, every row read back, and a second attach opens nothing", async ({ browser }) => {
  const recorded = { newContext: () => browser.newContext({ recordVideo: { dir: VIDEO ?? "", size: SIZE }, viewport: { width: 1440, height: 900 } }) } as typeof browser;
  editor = await signIn(VIDEO ? recorded : browser, EDITOR, `/projects/${PROJECT}?lang=en`);
  const page = editor.page;
  // Signed in wide: the sign-in's landmark is the side navigation, folded away on a phone.
  await page.setViewportSize(SIZE);
  await page.getByRole("button", { name: "Open the assistant" }).click();
  await pace(page);
  await attach(page);
  const dock = page.getByTestId("assistant-table");
  await pace(page);
  await shot(page, "1-attached");

  await dock.locator("#complete-btn").click();
  const review = dock.getByTestId("model-review");
  await expect(review).toBeVisible({ timeout: 60_000 });
  await expect(review.getByRole("combobox").first()).toBeVisible();
  await expect(dock.getByText("Check failed")).toHaveCount(0);
  await pace(page, 3_000);
  await shot(page, "2-model-proposed");

  const proposing = page.waitForResponse((r) => r.url().includes("jc_space_complete"), { timeout: 120_000 });
  await dock.locator("#complete-propose").click();
  const out = (await (await proposing).json()) as { drafts?: { kind: string; manifest: { spec?: { slug?: string } } }[] };
  const slug = out.drafts?.find((d) => d.kind === "Endpoint")?.manifest.spec?.slug ?? "";
  expect(slug).not.toBe("");
  const change = await proposedChange(page);
  await pace(page);
  await shot(page, "3-proposed");

  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  await approve(approver.page, PROJECT, change, SPACE);
  await approver.context.close();

  const write = dock.getByRole("button", { name: new RegExp(`Write ${ROWS} rows`) });
  await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 300_000 });
  await pace(page);
  await shot(page, "4-live");
  await write.click();
  await expect(dock.getByRole("status").filter({ hasText: `${ROWS} rows written, none refused.` })).toBeVisible({ timeout: 180_000 });
  await pace(page, 3_000);
  await shot(page, "5-rows-loaded");

  const read = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${TYPE}&count=true&limit=0`);
  expect(read.status(), await read.text()).toBe(200);
  expect(Number(read.headers()["ngsild-results-count"])).toBe(ROWS);

  // The same file again: nothing new to approve, and the same rows, not a copy of them.
  await dock.getByRole("button", { name: "Back to the assistant" }).click();
  await attach(page);
  await dock.locator("#complete-btn").click();
  const again = dock.getByRole("button", { name: new RegExp(`Write ${ROWS} rows`) });
  await expect(again).toBeVisible({ timeout: 60_000 });
  await expect(dock.locator("#complete-propose")).toHaveCount(0);
  await expect(again).not.toHaveAttribute("aria-disabled", "true", { timeout: 60_000 });
  await again.click();
  await expect(dock.getByRole("status").filter({ hasText: `${ROWS} rows written, none refused.` })).toBeVisible({ timeout: 180_000 });
  await shot(page, "6-second-attach");
  const twice = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${TYPE}&count=true&limit=0`);
  expect(Number(twice.headers()["ngsild-results-count"])).toBe(ROWS);
  if (VIDEO) writeFileSync(join(VIDEO, "space.txt"), SPACE);
});
