/**
 * A vendor's space in 60 seconds (T-6450, TS-19): a person with a project editor's rights signs in,
 * takes Start from a file on the Context Spaces list, gives five records, proposes the model, the
 * space, its Endpoint and their write grant as one change; the approver approves; the rows go in
 * and are read back through the Endpoint. The clock runs from the sign-in form. Over 60 s is
 * reported, not failed: the approver's click and the reconciler's run are the named blockers in
 * the task. Run with SHOTS_DIR set to keep the 1440 and 400 pictures and the laps.
 *
 * Names are `t6450-…`, so the janitor sweeps what a dead run leaves.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, EDITOR, STEWARD, VIEWER, approve, cleanUp, csrf, proposedChange, rejectOpen, removeCompletely, signIn, sweepDrafts, waitGone } from "./portal";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
const SPACE = `t6450-vendor-${Date.now().toString().slice(-6)}`;
const TYPE = "VendorOffer";
const ROWS = 5;
const TARGET_S = 60;
const SHOTS = process.env.SHOTS_DIR;
/** Five offers of a market stall vendor: invented, no personal data. */
const CSV = [
  "name,category,price,lat,lon",
  "Rye bread,bakery,4.2,60.1689,24.9316",
  "Sea buckthorn jam,preserves,7.5,60.1709,24.9459",
  "Smoked salmon,fish,12.9,60.1726,24.9445",
  "Blueberry pie,bakery,6.1,60.1793,24.9510",
  "Birch syrup,preserves,9.8,60.1630,24.9150",
].join("\n");

const laps: { step: string; seconds: number }[] = [];
let lap = 0;
function took(step: string): void {
  laps.push({ step, seconds: Math.round((Date.now() - lap) / 100) / 10 });
  lap = Date.now();
}

/** Both widths of one state, full height so the sticky sidebar is not cut; off the clock. */
async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  const started = Date.now();
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    await page.setViewportSize({ width, height: Math.min(height, 4000) });
    await page.screenshot({ path: join(SHOTS, `${state}-${width}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  lap += Date.now() - started;
}

test.describe.configure({ mode: "serial" });

let editor: Session;

test.afterAll(async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}?lang=en`);
  await rejectOpen(steward, PROJECT, SPACE, "vendor_space_60s");
  for (const plural of ["endpoints", "policies"]) {
    const answer = await steward.page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
    const items = answer.ok() ? (((await answer.json()) as { items?: { metadata: { name: string } }[] }).items ?? []) : [];
    for (const { metadata } of items.filter((one) => one.metadata.name.startsWith(SPACE))) {
      await removeCompletely(steward, PROJECT, plural, metadata.name);
      await waitGone(steward.page, PROJECT, plural, metadata.name);
    }
  }
  await cleanUp(steward, PROJECT, "spaces", SPACE, "vendor_space_60s");
  await waitGone(steward.page, PROJECT, "spaces", SPACE);
  await cleanUp(steward, PROJECT, "datamodels", SPACE, "vendor_space_60s");
  await sweepDrafts(editor.context, editor.page, PROJECT, new RegExp(`^${SPACE}`));
  await steward.context.close();
  await editor.context.close();
});

test("five records reach a new space and read back through its Endpoint", async ({ browser }) => {
  lap = Date.now();
  editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/spaces?lang=en`);
  const page = editor.page;
  took("Sign in, the Context Spaces list opens");
  await shot(page, "1-spaces-list");

  await page.getByRole("button", { name: "Start from a file" }).first().click();
  await page.locator('input[type="file"]').setInputFiles({ name: "offers.csv", mimeType: "text/csv", buffer: Buffer.from(CSV) });
  await page.getByRole("radio", { name: "My own model" }).check();
  await page.locator("#upload-own-type").fill(TYPE);
  await page.locator("#upload-space").fill(SPACE);
  await page.getByRole("option", { name: `New: “${SPACE}”` }).click();
  took("Start from a file: file, type name, space name");
  await shot(page, "2-file-chosen");

  const answered = page.waitForResponse((r) => r.url().includes("jc_space_complete") && r.request().method() === "POST");
  await page.getByRole("button", { name: "Propose the change" }).click();
  const out = (await (await answered).json()) as { drafts?: { kind: string; manifest: { spec?: { slug?: string } } }[] };
  const slug = out.drafts?.find((d) => d.kind === "Endpoint")?.manifest.spec?.slug ?? "";
  expect(slug).not.toBe("");
  const change = await proposedChange(page);
  took("Propose: model, space, Endpoint and write grant checked and proposed");
  await shot(page, "3-proposed");

  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  lap = Date.now();
  await approve(approver.page, PROJECT, change, SPACE);
  await approver.context.close();
  took("The approver approves (signing in stays off the clock)");

  const write = page.getByRole("button", { name: `Write ${ROWS} rows` });
  await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 300_000 });
  took("The grant goes live, the page unlocks the write by itself");
  await shot(page, "4-live");

  await write.click();
  await expect(page.getByRole("status").filter({ hasText: `${ROWS} rows written, none refused.` })).toBeVisible({ timeout: 60_000 });
  took("Write the rows");

  await expect
    .poll(async () => {
      const read = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${TYPE}&count=true&limit=0`);
      return read.ok() ? Number(read.headers()["ngsild-results-count"]) : read.status();
    })
    .toBe(ROWS);
  took("Read the count through the Endpoint");
  await shot(page, "5-rows-written");

  const total = Math.round(laps.reduce((sum, one) => sum + one.seconds, 0) * 10) / 10;
  console.log(JSON.stringify({ total, targetSeconds: TARGET_S, withinTarget: total <= TARGET_S, laps }, null, 1));
  if (SHOTS) writeFileSync(join(SHOTS, `timings-${Date.now()}.json`), JSON.stringify({ space: SPACE, total, laps }, null, 1));
});

test("a viewer reads why, and the op refuses them", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/spaces?lang=en`);
  await expect(viewer.page.getByRole("button", { name: /Start from a file/ }).first()).toHaveAttribute("aria-disabled", "true");
  await shot(viewer.page, "6-viewer-refused");
  const refused = await viewer.page.request.post(`/api/v1/projects/${PROJECT}/ops/jc_space_complete`, {
    headers: { "x-csrf-token": await csrf(viewer.context), "content-type": "application/json" },
    data: { space: `${SPACE}-v`, files: [{ name: "a.csv", content: "name\nx\n" }], upload: true, propose: true },
  });
  expect(refused.status()).toBe(403);
  await viewer.context.close();
});
