/**
 * The first dataset in ten minutes (T-5109), on dev, timed: a demo editor takes the home
 * checklist's first link (the CSV upload), reviews the model the Portal suggests from the
 * columns, proposes the space with its private Endpoint, the approver accepts it, the rows are
 * written, the editor publishes the Endpoint as a public dataset (typed confirmation), the
 * approver accepts that red change, and a visitor with no session reads the catalogue entry.
 * The clock runs from the editor's sign-in to the catalogue page; the budget is ten minutes.
 *
 * `tests/home_page.test.tsx` and `e2e/home.spec.ts` prove the checklist links here; helsinki
 * already holds published data, so its checklist is closed and this walk opens the link directly.
 * Names end in the run's HHMM so the janitor (T-2627) sweeps what a dead run leaves; `afterAll`
 * removes it too.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, EDITOR, STEWARD, approve, removeCompletely, proposedChange, signIn, sweepDrafts } from "./portal";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const SPACE = `fd-parking-${SUFFIX}`;
const MINE = new RegExp(`^fd-parking-${SUFFIX}`);
const BUDGET_MS = 10 * 60_000;
/** Ten car parks in Helsinki: the columns a municipality's export has. */
const CSV = [
  "name,lat,lon,capacity",
  "Kamppi,60.1689,24.9316,620",
  "Kluuvi,60.1709,24.9459,500",
  "Kaisaniemi,60.1726,24.9445,180",
  "Hakaniemi,60.1793,24.9510,240",
  "Ruoholahti,60.1630,24.9150,300",
  "Pasila,60.1990,24.9330,820",
  "Kalasatama,60.1870,24.9770,410",
  "Itäkeskus,60.2100,25.0820,1500",
  "Malmi,60.2510,25.0110,260",
  "Herttoniemi,60.1950,25.0300,350",
].join("\n");

let editor: Session;
let approver: Session;

test.describe.configure({ mode: "serial" });
test.use({ trace: "off" });

/** Approves a change as the approver, typing the resource's name back when the page asks (CC-19). */
async function approveAsked(project: string, change: string): Promise<void> {
  const { page } = approver;
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const typed = page.locator("#confirm-resource-name");
  const asked = await typed
    .waitFor({ state: "visible", timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
  await approve(page, project, change, asked ? ((await typed.getAttribute("placeholder")) ?? "") : undefined);
}

async function names(page: Page, plural: string): Promise<{ name: string; space?: string }[]> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
  if (!answer.ok()) return [];
  const items = ((await answer.json()) as { items?: { metadata: { name: string }; spec?: { contextSpaceRef?: unknown } }[] }).items ?? [];
  return items.map((item) => ({
    name: item.metadata.name,
    space: typeof item.spec?.contextSpaceRef === "string" ? item.spec.contextSpaceRef : (item.spec?.contextSpaceRef as { name?: string } | undefined)?.name,
  }));
}

test.afterAll(async ({ browser }) => {
  await editor?.context.close();
  await approver?.context.close();
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/home?lang=en`);
  try {
    // Dependents first, so no removal meets a reference (409).
    for (const plural of ["dashboards", "layers", "endpoints", "projections", "policies", "serviceaccounts", "spaces", "datamodels"]) {
      for (const { name, space } of await names(steward.page, plural)) {
        if (MINE.test(name) || space === SPACE) await removeCompletely(steward, PROJECT, plural, name);
      }
    }
    await sweepDrafts(steward.context, steward.page, PROJECT, MINE);
  } finally {
    await steward.context.close();
  }
});

test("a new editor publishes a CSV as a public dataset with its map and catalogue entry in under ten minutes", async ({ browser }) => {
  test.setTimeout(20 * 60_000);
  const started = Date.now();
  const lap = (what: string) => test.info().annotations.push({ type: "lap", description: `${what}: ${Math.round((Date.now() - started) / 1000)} s` });

  editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/spaces/upload?lang=en`);
  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/home?lang=en`);
  const page = editor.page;
  await expect(page.getByRole("heading", { level: 1, name: "Upload a CSV" })).toBeVisible();
  lap("signed in");

  // 1. The file, the suggested model reviewed (the first suggestion is preselected), the space.
  await page.getByLabel("Choose a CSV file").setInputFiles({ name: "parking.csv", mimeType: "text/csv", buffer: Buffer.from(CSV) });
  // The read waits for the Smart Data Models catalogue the columns are ranked against.
  await expect(page.getByText(/parking\.csv/)).toBeVisible({ timeout: 120_000 });
  const types = page.getByRole("radiogroup", { name: "What the rows are" });
  await expect(types).toBeVisible({ timeout: 30_000 });
  if (await page.getByRole("radio", { name: "My own model" }).isChecked()) {
    await page.locator("#upload-own-type").fill("CarPark");
  }
  const space = page.locator("#upload-space");
  await space.fill(SPACE);
  await page.getByRole("option", { name: `New: “${SPACE}”` }).click();
  await page.getByRole("button", { name: "Propose the change" }).click();
  await approveAsked(PROJECT, await proposedChange(page));
  lap("space, model and private endpoint approved");

  // 2. The Endpoint the upload proposed is private to the organization until the person chooses.
  let endpoint = "";
  await expect
    .poll(async () => {
      endpoint = (await names(page, "endpoints")).find((one) => one.space === SPACE)?.name ?? "";
      return endpoint;
    }, { timeout: 300_000, intervals: [5_000], message: "the upload's Endpoint reaches the mirror" })
    .not.toBe("");
  const audience = ((await (await page.request.get(`/api/v1/projects/${PROJECT}/endpoints/${endpoint}`)).json()) as { spec?: { audience?: string } }).spec?.audience;
  expect(audience, "the upload's Endpoint is not public until the person chooses it").not.toBe("public");

  // The page holds the write until the Change is Applied, so the first click writes.
  const write = page.getByRole("button", { name: /^Write \d+ rows?$/ });
  await expect(write).not.toHaveAttribute("aria-disabled", "true", { timeout: 300_000 });
  await write.click();
  await expect(page.getByText("10 rows written, none refused.")).toBeVisible({ timeout: 120_000 });
  lap("rows written");

  // 3. Public by an explicit choice: the dialog asks for the Endpoint's name typed back.
  await page.goto(`/projects/${PROJECT}/endpoints/${endpoint}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Publish a dataset" }).click();
  const dialog = page.getByRole("dialog", { name: "Publish a dataset" });
  await dialog.getByRole("button", { name: "Draft the description" }).click();
  const licence = dialog.locator("#publish-licence");
  await expect(licence).toBeVisible({ timeout: 120_000 });
  if (!(await licence.inputValue())) await licence.selectOption({ index: 1 });
  await dialog.getByRole("button", { name: "Preview the entry" }).click();
  const confirm = dialog.locator("#publish-confirm");
  await expect(dialog.getByRole("button", { name: "Make public and propose publication" })).toBeDisabled();
  await confirm.fill(endpoint);
  await dialog.getByRole("button", { name: "Make public and propose publication" }).click();
  await approveAsked(PROJECT, await proposedChange(page));
  lap("publication approved");

  // 4. A visitor with no session reads the entry; its map is the space's dashboard.
  const visitor = await browser.newContext();
  try {
    const reader = await visitor.newPage();
    await expect
      .poll(async () => {
        await reader.goto(`/catalogue/${endpoint}?lang=en`, { waitUntil: "load" });
        return reader.getByRole("heading", { name: "Resources" }).count();
      }, { timeout: 600_000, intervals: [15_000], message: "the dataset's page reaches the catalogue" })
      .toBeGreaterThan(0);
  } finally {
    await visitor.close();
  }
  lap("catalogue entry live");
  const elapsed = Date.now() - started;
  expect((await page.request.get(`/api/v1/projects/${PROJECT}/dashboards/${SPACE}`)).status(), "the upload drew the rows on a map").toBe(200);
  expect(elapsed, `the first dataset took ${Math.round(elapsed / 1000)} s`).toBeLessThan(BUDGET_MS);
});
