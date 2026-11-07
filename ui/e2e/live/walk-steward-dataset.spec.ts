/**
 * T-5536 — a department data steward's dataset from first upload to retirement, on dev, timed.
 *
 * Persona map (decided by worker-t5536, owner may override): a throwaway project with its own walk
 * people needs an org-admin and Keycloak users (T-5535's walk), so this walk runs in helsinki under
 * names starting `walk-steward-` and plays the departments with the seeded demo people:
 *   - the department's steward is demo.editor: proposes everything and approves nothing (PF-58);
 *   - the approver is demo.approver (TOTP, APPROVER_TOTP);
 *   - the organization admin is demo.steward (TOTP, PORTAL_TOTP): writes the Group and approves the
 *     removals, which need `delete` (see `removeCompletely`);
 *   - the second department is a Group `walk-steward-finance-<HHMM>` whose one member is demo.viewer;
 *   - the third department is demo.probe, who holds no grant on the space.
 *
 * Steps, each lapped (seconds since the steward's sign-in), with clicks and every Change's lane:
 *   1. upload a 500-row CSV, keep or name the suggested model;
 *   2. the private Endpoint answers 401 anonymous, 200 to the steward, 403 to the third department;
 *   3. a Policy naming the second department's Group: they read it, the third still gets 403;
 *   4. publish as a public dataset (red lane, typed name) until the CKAN entry is live; anonymous
 *      `file.csv` returns the 500 rows;
 *   5. upload a corrected CSV (changed capacities, one new column `chargers`) into the same space:
 *      the new column reaches `file.csv` and NGSI-LD;
 *   6. withdraw from public (audience back to the organization), then remove the space and all it
 *      holds;
 *   7. CKAN has no `walk-steward` package and the Endpoint's slug answers 404.
 * The step table is attached to the report as `walk-steward.json`; `SHOTS_DIR` collects 1440 and
 * 400 shots of each state. afterAll removes every `walk-steward-*` resource whatever step failed.
 *
 *   PORTAL_PASSWORD=… PORTAL_TOTP=… APPROVER_PASSWORD=… APPROVER_TOTP=… EDITOR_PASSWORD=… \
 *   VIEWER_PASSWORD=… PROBE_PASSWORD=… SHOTS_DIR=… \
 *     npx playwright test --config playwright.live.config.ts e2e/live/walk-steward-dataset.spec.ts
 */
import { expect, test } from "@playwright/test";
import type { APIRequestContext, Browser, BrowserContext, Locator, Page } from "@playwright/test";
import { APPROVER, EDITOR, PROBE, STEWARD, VIEWER, approve, checkManifest, csrf, proposedChange, removeCompletely, signIn, sweepDrafts } from "./portal";
import { shot } from "./shoot";

type Session = { context: BrowserContext; page: Page };
type Row = { step: string; lapS: number; clicks: number; changes: string[]; verdict: string };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const PREFIX = "walk-steward-";
const SPACE = `${PREFIX}environment-${SUFFIX}`;
const GROUP = `${PREFIX}finance-${SUFFIX}`;
const POLICY = `${PREFIX}finance-read-${SUFFIX}`;
const MINE = new RegExp(`^${PREFIX}`);
const ROWS = 500;
const CATALOGUE = (process.env.CKAN_URL ?? "https://opendata.dev.joinedcontext.com").replace(/\/$/, "");

/** 500 car parks on a grid over Helsinki; `corrected` changes every capacity and adds `chargers`. */
function carParks(corrected: boolean): string {
  const lines = [corrected ? "name,lat,lon,capacity,chargers" : "name,lat,lon,capacity"];
  for (let i = 0; i < ROWS; i++) {
    const lat = (60.15 + Math.floor(i / 25) * 0.005).toFixed(4);
    const lon = (24.85 + (i % 25) * 0.01).toFixed(4);
    const capacity = 50 + ((i * 37) % 900) + (corrected ? 5 : 0);
    lines.push([`Walk car park ${String(i + 1).padStart(3, "0")}`, lat, lon, capacity, ...(corrected ? [i % 7] : [])].join(","));
  }
  return lines.join("\n");
}

const rows: Row[] = [];
let started = 0;
let clicks = 0;
let changes: string[] = [];
let editor: Session;
let approver: Session;
let admin: Session;
let endpoint = "";
let slug = "";

test.describe.configure({ mode: "serial" });
test.use({ trace: "off" });

async function click(target: Locator): Promise<void> {
  clicks++;
  await target.click();
}

/** Closes a step: its lap, the clicks and Changes it took, and what it proved. */
function lap(step: string, verdict: string): void {
  rows.push({ step, lapS: Math.round((Date.now() - started) / 1000), clicks, changes, verdict });
  test.info().annotations.push({ type: "lap", description: `${step}: ${rows.at(-1)?.lapS} s, ${clicks} clicks, ${changes.join(" ") || "no change"}` });
  clicks = 0;
  changes = [];
}

/** Both widths of one state; `who` is the shot's audience label (shoot.ts). */
async function shots(page: Page, route: string, state: string, who: "steward" | "viewer" | "anonymous"): Promise<void> {
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: size.height });
    await shot(page, { area: "walk-steward", route, state, who, lang: "en" });
  }
  await page.setViewportSize(size);
}

/** The approver accepts a change, typing the name back when the lane is red; records id and lane. */
async function approveRecorded(change: string): Promise<void> {
  const detail = await approver.page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
  const lane = detail.ok() ? ((await detail.json()) as { status?: { lane?: string } }).status?.lane : undefined;
  changes.push(`${change}:${lane ?? "?"}`);
  await approver.page.goto(`/projects/${PROJECT}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const typed = approver.page.locator("#confirm-resource-name");
  const asked = await typed.waitFor({ state: "visible", timeout: 15_000 }).then(() => true, () => false);
  await approve(approver.page, PROJECT, change, asked ? ((await typed.getAttribute("placeholder")) ?? "") : undefined);
}

/** Proposes a manifest through its resource route as `who`; answers the Change's id. */
async function propose(who: Session, project: string, plural: string, manifest: { kind: string; metadata: { name: string; namespace?: string } } & Record<string, unknown>, method: "post" | "put" = "post"): Promise<string> {
  await checkManifest(who.page, who.context, project, manifest);
  const at = `/api/v1/projects/${project}/${plural}${method === "put" ? `/${manifest.metadata.name}` : ""}`;
  const answer = await who.page.request[method](at, { headers: { "x-csrf-token": await csrf(who.context) }, data: manifest });
  expect(answer.status(), await answer.text()).toBe(202);
  return ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
}

/** The status an Endpoint read answers this caller (its type list, the cheapest read). */
async function readStatus(request: APIRequestContext): Promise<number> {
  return (await request.get(`/api/endpoint/${slug}/ngsi-ld/v1/types`)).status();
}

async function anonymous<T>(browser: Browser, run: (request: APIRequestContext) => Promise<T>): Promise<T> {
  const context = await browser.newContext();
  try {
    return await run(context.request);
  } finally {
    await context.close();
  }
}

async function packages(request: APIRequestContext): Promise<number> {
  const answer = await request.get(`${CATALOGUE}/api/3/action/package_search?q=${encodeURIComponent(PREFIX.slice(0, -1))}&rows=0`);
  expect(answer.ok(), `package_search: ${answer.status()}`).toBe(true);
  return ((await answer.json()) as { result?: { count?: number } }).result?.count ?? -1;
}

/** Uploads one CSV into SPACE, proposes and approves what the page proposes, writes the rows. */
async function upload(csv: string, first: boolean): Promise<string> {
  const page = editor.page;
  await page.goto(`/projects/${PROJECT}/spaces/upload?lang=en`, { waitUntil: "load" });
  await expect(page.getByRole("heading", { level: 1, name: "Upload a CSV" })).toBeVisible();
  if (first) await shots(page, "/projects/helsinki/spaces/upload", "empty", "steward");
  await page.getByLabel("Choose a CSV file").setInputFiles({ name: "car-parks.csv", mimeType: "text/csv", buffer: Buffer.from(csv) });
  await expect(page.getByText(/car-parks\.csv/)).toBeVisible({ timeout: 120_000 });
  await expect(page.getByRole("radiogroup", { name: "What the rows are" })).toBeVisible({ timeout: 30_000 });
  if (await page.getByRole("radio", { name: "My own model" }).isChecked()) await page.locator("#upload-own-type").fill("CarPark");
  await page.locator("#upload-space").fill(SPACE);
  await click(page.getByRole("option", { name: first ? `New: “${SPACE}”` : SPACE, exact: !first }));
  await shots(page, "/projects/helsinki/spaces/upload", first ? "filled" : "correction", "steward");
  const proposeButton = page.getByRole("button", { name: "Propose the change" });
  let decided = "nothing to re-decide";
  if (await proposeButton.isVisible()) {
    await click(proposeButton);
    await approveRecorded(await proposedChange(page));
    decided = "a change approved";
  }
  const write = page.getByRole("button", { name: /^Write \d+ rows?$/ });
  await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 300_000 });
  await click(write);
  await expect(page.getByText(`${ROWS} rows written, none refused.`)).toBeVisible({ timeout: 300_000 });
  return decided;
}

async function csvOf(request: APIRequestContext): Promise<string[]> {
  const types = ((await (await request.get(`/api/endpoint/${slug}/ngsi-ld/v1/types`)).json()) as { typeList?: string[] }).typeList ?? [];
  expect(types.length, "the space holds one type").toBe(1);
  const answer = await request.get(`/api/endpoint/${slug}/file.csv?type=${encodeURIComponent(types[0])}`);
  expect(answer.status(), "anonymous file.csv").toBe(200);
  return (await answer.text()).trim().split(/\r?\n/);
}

/** Removes everything named walk-steward-* or living in SPACE, dependents first, as the admin. */
async function cleanUp(owner: Session): Promise<void> {
  for (const plural of ["dashboards", "layers", "endpoints", "policies", "projections", "spaces", "datamodels"]) {
    const answer = await owner.page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
    if (!answer.ok()) continue;
    const items = ((await answer.json()) as { items?: { metadata: { name: string }; spec?: { contextSpaceRef?: string | { name?: string } } }[] }).items ?? [];
    for (const item of items) {
      const ref = item.spec?.contextSpaceRef;
      const space = typeof ref === "string" ? ref : ref?.name;
      if (MINE.test(item.metadata.name) || space === SPACE) await removeCompletely(owner, PROJECT, plural, item.metadata.name);
    }
  }
  await removeCompletely(owner, "org", "groups", GROUP);
  await sweepDrafts(owner.context, owner.page, PROJECT, MINE);
}

test.afterAll(async ({ browser }) => {
  test.info().attachments.push({ name: "walk-steward.json", contentType: "application/json", body: Buffer.from(JSON.stringify(rows, null, 2)) });
  console.log(JSON.stringify(rows));
  const owner = admin ?? (await signIn(browser, STEWARD, `/projects/${PROJECT}/home?lang=en`));
  try {
    await cleanUp(owner);
    expect(await packages(owner.page.request), "a walk-steward package is left in CKAN").toBe(0);
  } finally {
    for (const one of [editor, approver, owner]) await one?.context.close();
  }
});

test("a department steward publishes, corrects, withdraws and retires a 500-row dataset", async ({ browser }) => {
  test.setTimeout(60 * 60_000);
  started = Date.now();
  editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/home?lang=en`);
  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/home?lang=en`);
  admin = await signIn(browser, STEWARD, `/projects/${PROJECT}/home?lang=en`);
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/home?lang=en`);
  const probe = await signIn(browser, PROBE, `/projects/${PROJECT}/home?lang=en`);
  try {
    // 1. The upload: model suggested from the columns, the space and its Endpoint, the rows.
    lap("1. upload 500 rows", await upload(carParks(false), true));

    // 2. The Endpoint the upload proposed is private until the steward chooses otherwise.
    await expect
      .poll(async () => {
        const answer = await editor.page.request.get(`/api/v1/projects/${PROJECT}/endpoints`);
        const items = ((await answer.json()) as { items?: { metadata: { name: string }; spec?: { contextSpaceRef?: string | { name?: string }; slug?: string } }[] }).items ?? [];
        const mine = items.find((item) => [item.spec?.contextSpaceRef, (item.spec?.contextSpaceRef as { name?: string } | undefined)?.name].includes(SPACE));
        endpoint = mine?.metadata.name ?? "";
        slug = mine?.spec?.slug ?? "";
        return slug;
      }, { timeout: 300_000, intervals: [5_000], message: "the upload's Endpoint reaches the mirror" })
      .not.toBe("");
    await editor.page.goto(`/projects/${PROJECT}/endpoints/${endpoint}?lang=en`, { waitUntil: "load" });
    await shots(editor.page, `/projects/helsinki/endpoints/${endpoint}`, "private", "steward");
    expect([401, 404], "anonymous reads the private Endpoint").toContain(await anonymous(browser, readStatus));
    expect(await readStatus(editor.page.request), "the steward reads their own Endpoint").toBe(200);
    expect(await readStatus(probe.page.request), "the third department reads the private Endpoint").toBe(403);
    lap("2. private Endpoint", "401 anonymous, 200 steward, 403 third department");

    // 3. The second department reads through a Policy naming its Group; the third still does not.
    changes.push(`${await propose(admin, "org", "groups", {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Group",
      metadata: { name: GROUP, namespace: "org" },
      spec: { description: "The finance department of the T-5536 walk", members: [{ user: VIEWER.user }] },
    })}:org-admin`);
    await approve(admin.page, "org", changes.at(-1)?.split(":")[0] ?? "", GROUP);
    await approveRecorded(await propose(editor, PROJECT, "policies", {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: { name: POLICY, namespace: PROJECT },
      spec: { contextSpaceRef: { kind: "ContextSpace", name: SPACE }, assigner: "did:web:{orgDomain}", assignee: { kind: "group", id: GROUP }, operations: ["retrieveOps"] },
    }));
    // A fresh sign-in, so the viewer's token carries the group once Keycloak has it.
    await expect
      .poll(async () => {
        const fresh = await signIn(browser, VIEWER, `/projects/${PROJECT}/home?lang=en`);
        try {
          return await readStatus(fresh.page.request);
        } finally {
          await fresh.context.close();
        }
      }, { timeout: 600_000, intervals: [20_000], message: "the second department reads through its Group" })
      .toBe(200);
    expect(await readStatus(probe.page.request), "the third department after the grant").toBe(403);
    lap("3. second department granted", "group member 200, third department 403");

    // 4. Public by an explicit choice, a red change the approver accepts; then CKAN lists it.
    const page = editor.page;
    await page.goto(`/projects/${PROJECT}/endpoints/${endpoint}?lang=en`, { waitUntil: "load" });
    await click(page.getByRole("button", { name: "Publish a dataset" }));
    const dialog = page.getByRole("dialog", { name: "Publish a dataset" });
    await click(dialog.getByRole("button", { name: "Draft the description" }));
    const licence = dialog.locator("#publish-licence");
    await expect(licence).toBeVisible({ timeout: 120_000 });
    if (!(await licence.inputValue())) await licence.selectOption({ index: 1 });
    await click(dialog.getByRole("button", { name: "Preview the entry" }));
    await expect(dialog.getByRole("button", { name: "Make public and propose publication" }), "refused until the name is typed").toBeDisabled();
    await shots(page, `/projects/helsinki/endpoints/${endpoint}`, "publish-refused-untyped", "steward");
    await dialog.locator("#publish-confirm").fill(endpoint);
    await click(dialog.getByRole("button", { name: "Make public and propose publication" }));
    const publication = await proposedChange(page);
    // The steward may not approve their own public exposure (PF-58).
    await page.goto(`/projects/${PROJECT}/approvals/${publication}?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("button", { name: "Approve", exact: true })).toBeDisabled({ timeout: 60_000 });
    await shots(page, `/projects/helsinki/approvals/${publication}`, "own-approval-refused", "steward");
    const approvedAt = Date.now();
    await approveRecorded(publication);
    await expect
      .poll(() => anonymous(browser, packages), { timeout: 900_000, intervals: [15_000], message: "the CKAN entry goes live" })
      .toBe(1);
    test.info().annotations.push({ type: "catalogue", description: `approval to catalogue live: ${Math.round((Date.now() - approvedAt) / 1000)} s; upload to catalogue live: ${Math.round((Date.now() - started) / 1000)} s` });
    expect((await anonymous(browser, csvOf)).length - 1, "anonymous file.csv rows").toBe(ROWS);
    const visitor = await browser.newContext();
    try {
      const reader = await visitor.newPage();
      await reader.goto(`/catalogue/${endpoint}?lang=en`, { waitUntil: "load" });
      await shots(reader, `/catalogue/${endpoint}`, "public", "anonymous");
    } finally {
      await visitor.close();
    }
    lap("4. published with CKAN entry", "red lane approved by another person, 500 rows anonymous");

    // 5. The corrected file: changed capacities and one new column, in CSV and NGSI-LD.
    const decided = await upload(carParks(true), false);
    await expect
      .poll(async () => (await anonymous(browser, csvOf))[0], { timeout: 300_000, intervals: [10_000], message: "the new column reaches file.csv" })
      .toContain("chargers");
    const types = ((await (await editor.page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/types`)).json()) as { typeList?: string[] }).typeList ?? [];
    const one = await editor.page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${encodeURIComponent(types[0] ?? "")}&limit=1&format=keyValues`);
    expect(Object.keys(((await one.json()) as Record<string, unknown>[])[0] ?? {}), "NGSI-LD carries the new column").toContain("chargers");
    lap("5. corrected CSV", decided);

    // 6. Withdrawn from public, then the space and all it holds removed.
    const current = (await (await page.request.get(`/api/v1/projects/${PROJECT}/endpoints/${endpoint}`)).json()) as { apiVersion: string; kind: string; metadata: { name: string; namespace?: string }; spec: Record<string, unknown> };
    await approveRecorded(await propose(editor, PROJECT, "endpoints", { apiVersion: current.apiVersion, kind: current.kind, metadata: { name: endpoint, namespace: PROJECT }, spec: { ...current.spec, audience: "organization" } }, "put"));
    await expect
      .poll(() => anonymous(browser, readStatus), { timeout: 300_000, intervals: [10_000], message: "anonymous loses the withdrawn Endpoint" })
      .toBeGreaterThanOrEqual(401);
    await expect
      .poll(() => anonymous(browser, packages), { timeout: 900_000, intervals: [15_000], message: "the CKAN entry is withdrawn" })
      .toBe(0);
    await shots(page, `/projects/helsinki/endpoints/${endpoint}`, "withdrawn", "steward");
    await cleanUp(admin);
    lap("6. withdrawn and removed", "audience organization, space removed");

    // 7. Nothing left: no package, the slug answers 404.
    expect(await anonymous(browser, packages), "CKAN packages named walk-steward").toBe(0);
    await expect
      .poll(() => readStatus(editor.page.request), { timeout: 300_000, intervals: [10_000], message: "the removed Endpoint's slug" })
      .toBe(404);
    expect(await readStatus(probe.page.request), "the third department never read the data").not.toBe(200);
    lap("7. retired", "CKAN 0, slug 404");
  } finally {
    await viewer.context.close();
    await probe.context.close();
  }
});
