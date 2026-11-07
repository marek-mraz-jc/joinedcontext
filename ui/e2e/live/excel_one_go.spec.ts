/**
 * One workbook, one go (T-6445): a person with only a project editor's rights gives Complete this
 * space one .xlsx, the approver approves the bundle, and the same page loads every row of the
 * chosen sheet without asking for the file again. The count through the space's Endpoint is the
 * proof. Run with SHOTS_DIR set to keep the 1440 and 400 pictures and the timings of each step.
 *
 * `EXCEL_VIDEO=<dir>` records the presentation take (T-6567) on Upload a CSV: the file read and its
 * model drawn, one Propose, the approval, all rows written, the rows on the space's page, and two
 * questions the assistant answers from them. `cuts.json` names the waits `excel-video.sh` cuts.
 *
 * Names are `t6567-…`, so the janitor sweeps what a dead run leaves. The rows go too, while the
 * Endpoint stands (T-6471): see `removeRows`.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import { APPROVER, EDITOR, STEWARD, VIEWER, approve, ask, csrf, proposedChange, rejectOpen, removeCompletely, signIn, sweepDrafts } from "./portal";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
/** Example data (supervisor 2026-11-03): 21 playgrounds of Banská Bystrica on one sheet. */
const WORKBOOK = join(dirname(fileURLToPath(import.meta.url)), "fixtures/detske-ihriska-banska-bystrica-priklad.xlsx");
/** The type both pages name after the sheet `Detské ihriská`. */
const TYPE = "DetskeIhriska";
const ROWS = 21;
const SPACE = `t6567-ihriska-${Date.now().toString().slice(-6)}`;
const SHOTS = process.env.SHOTS_DIR;
const VIDEO = process.env.EXCEL_VIDEO;
/** The two questions of the take, answered from the rows: one playground is `zlý`, one on Fončorda is barrier-free. */
const QUESTIONS = ["Koľko ihrísk je v stave zlý?", "Ktoré ihriská sú bezbariérové na Fončorde?"];
/** What each answer must name, read off the sheet. */
const ANSWERS = [/\b1\b|jedno|jeden/i, /Tulsk/];
/** Words a presentation must not show: a tool's own line, a refusal, a token count. */
const NOISE = /tokens|jc_[a-z_]+|is not a read tool|Details \(|does not use navigate/i;
const marks: Record<string, number> = {};
/** The waits cut from the take, as seconds of the editor's video. */
const cuts: [number, number][] = [];
let started = 0;
const now = () => Math.round((Date.now() - started) / 100) / 10;
const mark = (name: string) => {
  marks[name] = now();
};
const hold = async (page: Page, ms = 2_500) => {
  if (VIDEO) await page.waitForTimeout(ms);
};

/** What the assistant answered to `question`, once it stopped working and growing. */
async function settledAnswer(page: Page, question: string): Promise<string> {
  const chat = page.locator("#run-chat");
  let text = "";
  await expect
    .poll(
      async () => {
        const now = await chat.innerText();
        const after = now.slice(now.lastIndexOf(question) + question.length).trim();
        const same = after === text && after.length > 40 && (await page.getByTestId("assistant-busy").count()) === 0;
        text = after;
        return same;
      },
      { timeout: 240_000, intervals: [6_000] },
    )
    .toBe(true);
  return text;
}

const timings: { step: string; seconds: number; clicks: number }[] = [];
let lap = Date.now();
function took(step: string, clicks: number): void {
  timings.push({ step, seconds: Math.round((Date.now() - lap) / 100) / 10, clicks });
  lap = Date.now();
}

/** Both widths of one state, full height so the sticky sidebar is not cut. */
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
let slug = "";

/**
 * Takes the run's rows out of the broker (T-6471). A space is its own broker Tenant, which outlives
 * the space, so rows left here stay with nothing to reach them. The upload grant holds no
 * `deleteBatch` on purpose: a person's load is no licence to delete, and the journey does not widen
 * it. The steward grants this run's editor the delete in a Policy of its own, which goes with the
 * space's other Policies, and the rows leave through the Endpoint once a sync has served it.
 */
async function removeRows(steward: Session): Promise<void> {
  const rows = `/api/endpoint/${slug}/ngsi-ld/v1`;
  const count = async () => {
    const read = await editor.page.request.get(`${rows}/entities?type=${TYPE}&count=true&limit=0`);
    expect(read.status(), await read.text()).toBe(200);
    return Number(read.headers()["ngsild-results-count"]);
  };
  if (!slug || (await count()) === 0) return;

  const upload = await steward.page.request.get(`/api/v1/projects/${PROJECT}/policies/${SPACE}-upload`);
  expect(upload.status(), await upload.text()).toBe(200);
  const { assignee } = ((await upload.json()) as { spec: { assignee: unknown } }).spec;
  const proposed = await steward.page.request.post(`/api/v1/projects/${PROJECT}/policies`, {
    headers: { "x-csrf-token": await csrf(steward.context) },
    data: {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: { name: `${SPACE}-rows-delete`, namespace: PROJECT, title: { en: `The excel_one_go journey removes the rows of ${SPACE}` } },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: SPACE },
        assigner: "did:web:{orgDomain}",
        assignee,
        operations: ["queryBatch", "deleteBatch"],
        information: [{ entities: [{ type: TYPE }] }],
      },
    },
  });
  expect(proposed.status(), await proposed.text()).toBe(202);
  await approve(steward.page, PROJECT, ((await proposed.json()) as { metadata: { name: string } }).metadata.name, `${SPACE}-rows-delete`);

  const listed = await editor.page.request.get(`${rows}/entities?type=${TYPE}&limit=1000&pick=id`);
  expect(listed.status(), await listed.text()).toBe(200);
  const ids = ((await listed.json()) as { id: string }[]).map((one) => one.id);
  // The Tenant is this run's, so nothing else can be here; a row that says otherwise stops the journey.
  expect(ids.filter((id) => !id.startsWith(`urn:ngsi-ld:${TYPE}:`) || !id.includes(`-${SPACE}:`))).toEqual([]);
  expect(ids).toHaveLength(ROWS);
  const headers = { "x-csrf-token": await csrf(editor.context), "JC-Reason": "excel_one_go removes its own rows" };
  const remove = async (part: string[]) => (await editor.page.request.post(`${rows}/entityOperations/delete`, { headers, data: part })).status();
  const first = ids.slice(0, 100);
  await expect.poll(() => remove(first), { timeout: 300_000, intervals: [10_000], message: "the delete grant reaches the Endpoint" }).toBe(204);
  for (let at = 100; at < ids.length; at += 100) expect(await remove(ids.slice(at, at + 100))).toBe(204);
  expect(await count()).toBe(0);
}

/**
 * Leaves dev with no open Change, no rows, no grant, no space and no model of the run. A removal
 * that would not go fails the run after everything else is cleaned.
 */
test.afterAll(async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}?lang=en`);
  await rejectOpen(steward, PROJECT, SPACE, "excel_one_go");
  const rowsStayed = await removeRows(steward).then(
    () => undefined,
    (refused: unknown) => refused,
  );
  const refusals: unknown[] = rowsStayed ? [rowsStayed] : [];
  const mine = async (plural: string) => {
    const answer = await steward.page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
    expect(answer.status(), await answer.text()).toBe(200);
    return (((await answer.json()) as { items?: { metadata: { name: string } }[] }).items ?? []).map(({ metadata }) => metadata.name).filter((name) => name.startsWith(SPACE));
  };
  // Dependents first, and each kind gone from the Portal before the next: the space answers 409 while a grant still names it.
  for (const plural of ["endpoints", "policies", "spaces", "datamodels"]) {
    await (async () => {
      for (const name of await mine(plural)) await removeCompletely(steward, PROJECT, plural, name);
      await expect.poll(() => mine(plural), { timeout: 300_000, intervals: [5_000], message: `${plural} of ${SPACE} leave the Portal` }).toEqual([]);
    })().catch((refused: unknown) => refusals.push(refused));
  }
  await sweepDrafts(editor.context, editor.page, PROJECT, new RegExp(`^${SPACE}`));
  await steward.context.close();
  const video = editor.page.video();
  await editor.context.close();
  if (VIDEO) await video?.saveAs(join(VIDEO, "flow.webm"));
  if (refusals.length) throw refusals[0];
});

test.setTimeout(VIDEO ? 1_200_000 : 600_000);

test("one .xlsx makes the model, the space and its rows, read back through the Endpoint", async ({ browser }) => {
  test.skip(VIDEO !== undefined, "the recorded take runs the same load on Upload a CSV");
  editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/spaces/complete?lang=en`);
  const page = editor.page;
  lap = Date.now();
  await expect(page.getByRole("heading", { name: "Complete this space", level: 1 })).toBeVisible();
  await shot(page, "1-empty");

  await page.getByLabel("Files", { exact: true }).setInputFiles(WORKBOOK);
  await page.locator("#complete-space").fill(SPACE);
  await page.getByRole("heading", { name: "Complete this space", level: 1 }).click();
  await expect(page.getByText(`${ROWS} rows, 11 columns`)).toBeVisible();
  took("Choose the file and name the space", 2);
  await shot(page, "2-file-chosen");

  await page.locator("#complete-btn").click();
  await expect(page.locator("#complete-propose")).toBeVisible({ timeout: 60_000 });
  for (const kind of ["DataModel", "ContextSpace", "Endpoint"]) {
    await expect(page.getByTestId(`complete-draft-${kind}`)).toContainText("Check passed");
  }
  await expect(page.getByText("Check failed")).toHaveCount(0);
  // The drafted model drawn before Propose, each column with its kind of value (T-6567).
  const drawn = page.getByTestId("drafted-model");
  for (const word of ["Whole number", "Date", "Yes or no", "Place"]) await expect(drawn).toContainText(word);
  took("Complete: every draft checked, the model drawn", 1);
  await shot(page, "3-drafts-checked");

  const answered = page.waitForResponse((r) => r.url().includes("jc_space_complete"), { timeout: 120_000 });
  await page.locator("#complete-propose").click();
  const out = (await (await answered).json()) as { drafts?: { kind: string; manifest: { spec?: { slug?: string } } }[] };
  slug = out.drafts?.find((d) => d.kind === "Endpoint")?.manifest.spec?.slug ?? "";
  expect(slug, JSON.stringify(out).slice(0, 800)).not.toBe("");
  const change = await proposedChange(page);
  took("Propose all", 1);
  await shot(page, "4-proposed");

  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  lap = Date.now();
  await approve(approver.page, PROJECT, change, SPACE);
  await approver.context.close();
  took("The approver types the space name and approves (another person, their own page)", 1);

  const write = page.getByRole("button", { name: new RegExp(`Write ${ROWS} rows`) });
  await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 300_000 });
  took("The change goes live, the page unlocks the load by itself", 0);
  await shot(page, "5-approved");

  await write.click();
  await expect(page.getByRole("status").filter({ hasText: `${ROWS} rows written, none refused.` })).toBeVisible({ timeout: 180_000 });
  took("Load every row", 1);
  await shot(page, "6-rows-loaded");

  const read = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${TYPE}&count=true&limit=0`);
  expect(read.status(), await read.text()).toBe(200);
  expect(Number(read.headers()["ngsild-results-count"])).toBe(ROWS);
  took("Read the count through the Endpoint", 0);

  const total = timings.reduce((sum, one) => sum + one.seconds, 0);
  console.log(JSON.stringify({ total: Math.round(total * 10) / 10, clicks: timings.reduce((sum, one) => sum + one.clicks, 0), timings }, null, 1));
  if (SHOTS) writeFileSync(join(SHOTS, `timings-${Date.now()}.json`), JSON.stringify({ space: SPACE, total, timings }, null, 1));
});

/** The answer to `question` once the assistant stopped working, with what it wrote and nothing else. */
async function answerTo(page: Page, question: string): Promise<string> {
  const text = await settledAnswer(page, question);
  expect(text, "a presentation shows no tool lines, refusals or token counts").not.toMatch(NOISE);
  return text;
}

test("the presentation take: file, model, one approval, 21 rows, two answers (T-6567)", async ({ browser }) => {
  test.skip(VIDEO === undefined, "EXCEL_VIDEO=<dir> records it");
  const dir = VIDEO ?? "";
  const recorded = { newContext: () => browser.newContext({ recordVideo: { dir, size: { width: 1440, height: 900 } }, viewport: { width: 1440, height: 900 } }) } as Browser;
  started = Date.now();
  editor = await signIn(recorded, EDITOR, `/projects/${PROJECT}/spaces/upload?space=${SPACE}&lang=en`);
  const page = editor.page;
  // The sign-in is no part of the story.
  cuts.push([0, now()]);
  await expect(page.getByText("What this file becomes")).toBeVisible();
  await hold(page, 3_000);

  await page.getByLabel("Choose a CSV file or an Excel workbook (.xlsx)").setInputFiles(WORKBOOK);
  await expect(page.getByText(`${ROWS} rows, 11 columns`)).toBeVisible({ timeout: 60_000 });
  await expect(page.getByLabel("Type name")).toHaveValue(TYPE);
  const drawn = page.getByTestId("drafted-model");
  await expect(drawn).toBeVisible({ timeout: 30_000 });
  for (const word of ["Whole number", "Date", "Yes or no", "Place"]) await expect(drawn).toContainText(word);
  await drawn.scrollIntoViewIfNeeded();
  await hold(page, 6_000);
  mark("model drawn");

  const answered = page.waitForResponse((r) => r.url().includes("jc_space_complete") && r.request().postData()?.includes('"propose":true') === true, { timeout: 120_000 });
  await page.getByRole("button", { name: "Propose the change" }).click();
  const out = (await (await answered).json()) as { drafts?: { kind: string; manifest: { spec?: { slug?: string } } }[] };
  slug = out.drafts?.find((d) => d.kind === "Endpoint")?.manifest.spec?.slug ?? "";
  expect(slug, JSON.stringify(out).slice(0, 800)).not.toBe("");
  const change = await proposedChange(page);
  await hold(page, 3_000);
  mark("proposed");

  // Another person approves on their own page; the wait for the change to go live is cut.
  const waited = now();
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  await approve(approver.page, PROJECT, change, SPACE);
  await approver.context.close();
  const write = page.getByRole("button", { name: new RegExp(`Write ${ROWS} rows`) });
  await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 300_000 });
  cuts.push([waited, now()]);
  mark("live");
  await hold(page, 3_000);

  await write.click();
  const writing = now();
  const done = page.getByRole("status").filter({ hasText: `${ROWS} rows written, none refused.` });
  await expect(done).toBeVisible({ timeout: 180_000 });
  if (now() - writing > 4) cuts.push([writing + 1, now() - 1]);
  await hold(page, 4_000);
  mark("rows written");

  const read = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${TYPE}&count=true&limit=0`);
  expect(Number(read.headers()["ngsild-results-count"])).toBe(ROWS);

  // The rows on the space's own page: the table of entities with their map.
  const opening = now();
  await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);
  await expect(page.getByText("Detské ihrisko Tulská").first()).toBeVisible({ timeout: 60_000 });
  cuts.push([opening, now() - 0.5]);
  await hold(page, 6_000);
  mark("rows shown");

  const answers: { question: string; answer: string }[] = [];
  for (const [at, question] of QUESTIONS.entries()) {
    await ask(page, question);
    const asked = now();
    const answer = await answerTo(page, question);
    expect(answer).toMatch(ANSWERS[at]);
    // The model's thinking is cut; the question and the whole answer stay.
    if (now() - asked > 6) cuts.push([asked + 2, now() - 1]);
    answers.push({ question, answer });
    await hold(page, 7_000);
    await page.screenshot({ path: join(dir, `answer-${at + 1}-1440.png`) });
  }
  mark("answered");
  await page.setViewportSize({ width: 400, height: 844 });
  await hold(page, 1_000);
  await page.screenshot({ path: join(dir, "answer-2-400.png") });
  writeFileSync(join(dir, "answers.json"), JSON.stringify({ space: SPACE, marks, cuts, answers }, null, 1));
  writeFileSync(join(dir, "cuts.json"), JSON.stringify(cuts));
});

test("a viewer reads why, and the op refuses them", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/spaces/complete?lang=en`);
  await expect(viewer.page.getByRole("status").filter({ hasText: "Your role lets you look at these" })).toBeVisible();
  await expect(viewer.page.locator("#complete-btn")).toHaveCount(0);
  await shot(viewer.page, "7-viewer-refused");
  const refused = await viewer.page.request.post(`/api/v1/projects/${PROJECT}/ops/jc_space_complete`, {
    headers: { "x-csrf-token": await csrf(viewer.context), "content-type": "application/json" },
    data: { space: `${SPACE}-v`, files: [{ name: "a.csv", content: "name\nx\n" }], upload: true, propose: true },
  });
  expect(refused.status()).toBe(403);
  await viewer.context.close();
});
