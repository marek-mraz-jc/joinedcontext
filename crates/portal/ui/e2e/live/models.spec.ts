/**
 * The data model editor on dev (T-2729; DM-01, UI-16, TS-26): what a steward does to a model, each
 * as a person does it, and the viewer's refusal.
 *
 * - a new model in the structure editor: two classes, an enum with a value, a slot whose range is
 *   the enum, a one-to-many relationship, one line typed into the source, a Check, and Propose,
 *   whose Change the approver rejects;
 * - a model drafted from a CSV sample, and a Smart Data Model imported into a model being built;
 * - a removal proposed from the models' list, rejected, so the model stays.
 *
 * Nothing lands: every Change is rejected, the drafts the editor keeps are swept, and a sample
 * or an import that fills the editor is never saved. Every space holds exactly one model (DM-61), and
 * a new space is created with its own empty one (named after the space), so the run makes a sandbox
 * space and builds into that model, then removes the space at the end; its `t5066-` name lets
 * residue.spec.ts sweep it if a run dies first. The import starts from a model that already has a
 * class, because an import into an empty editor becomes a model that cannot be saved (T-2847).
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { guideShot } from "./guide";
import { APPROVER, STEWARD, VIEWER, approve, checkManifest, csrf, proposedChange, reject, removeCompletely, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
/** The space this run builds in (see above); its empty model carries the space's name. */
const SPACE = `t5066-${Date.now().toString(36)}`;
const MODEL = SPACE;
/** Every draft this run leaves: the space's model, and the one the sample fills, named after its file. */
const MINE = new RegExp(`^(jm-${SUFFIX}|${SPACE}$)`);
const SAMPLE = `jm-${SUFFIX}-sample`;
let sandbox: Promise<void> | undefined;

test.setTimeout(600_000);

/** Adds a class in the structure editor and leaves it the active one. */
async function addClass(page: Page, name: string): Promise<void> {
  await page.getByLabel("New type").fill(name);
  await page.getByRole("button", { name: "Add type" }).click();
  await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
}

/** The run's sandbox space, checked then proposed (PF-57), made once and approved by the steward when the lane asks (CC-67). */
async function sandboxSpace(page: Page): Promise<void> {
  const manifest = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: SPACE, namespace: PROJECT }, spec: { isSandbox: true } };
  await checkManifest(page, page.context(), PROJECT, manifest);
  const answer = await page.request.post(`/api/v1/projects/${PROJECT}/spaces`, {
    headers: { "x-csrf-token": await csrf(page.context()) },
    data: manifest,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = (await answer.json()) as { metadata: { name: string }; status: { phase: string } };
  if (change.status.phase === "PendingApproval") {
    await approve(page, PROJECT, change.metadata.name);
  }
  for (const plural of ["spaces", "datamodels"]) {
    await expect
      .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/${plural}/${SPACE}`)).status(), {
        message: `the sandbox ${plural} entry ${SPACE} exists`,
        timeout: 180_000,
        intervals: [3_000],
      })
      .toBe(200);
  }
}

test.afterAll(async ({ browser }) => {
  if (!sandbox) return;
  const owner = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  await removeCompletely(owner, PROJECT, "spaces", SPACE).catch(() => undefined);
  await owner.context.close();
});

/** The sandbox space's own empty model, open in the editor. */
async function newModel(page: Page): Promise<void> {
  sandbox ??= sandboxSpace(page);
  await sandbox;
  await page.goto(`/projects/${PROJECT}/models?edit=${encodeURIComponent(MODEL)}&lang=en`, { waitUntil: "load" });
  await expect(page.getByLabel("New type"), "the space's empty model opens in the editor").toBeVisible({ timeout: 60_000 });
}

test("a steward builds a model with classes, an enum and a has-many relation, edits its source, checks and proposes it", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/models?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  try {
    await newModel(page);

    await addClass(page, "Station");
    await addClass(page, "Sensor");
    await page.getByLabel("New enum").fill("QualityBand");
    await page.getByRole("button", { name: "Add enum" }).click();
    await page.getByLabel("New value of QualityBand").fill("good");
    await page.getByRole("button", { name: "Add value to QualityBand" }).click();

    // A slot of Station whose range is the enum.
    await page.getByRole("button", { name: "Station", exact: true }).click();
    await page.getByLabel("New field").fill("band");
    await page.getByRole("button", { name: "Add field" }).click();
    // T-4829 lists the fields as rows: the new one opens to its details, where "Type of value" is.
    await page.getByRole("button", { name: "band", exact: true }).click();
    await page.getByLabel("Type of value", { exact: true }).selectOption("QualityBand");

    // One Station has many Sensors: the relationship form writes both ends (multivalued on the owner).
    // The editor shows a relationship form per type; this is Station's.
    const relate = page.locator("form").filter({ has: page.getByRole("heading", { name: "Add a relationship from Station" }) });
    await relate.getByRole("radio", { name: "One to many" }).check();
    await relate.getByLabel("Target class", { exact: true }).selectOption("Sensor");
    await relate.getByRole("button", { name: "Add relationship" }).click();
    await expect(page.getByRole("table", { name: "Relationships" })).toContainText(/has many/);
    await guideShot(page, "model-1-classes-and-relationship");

    // One line typed into the source, where a person writes what the form has no control for.
    await page.getByRole("tab", { name: "Source" }).click();
    await page.getByRole("button", { name: "Edit source" }).click();
    const source = page.locator(".monaco-editor .view-lines").first();
    await expect(source).toBeVisible({ timeout: 60_000 });
    await source.click();
    await page.keyboard.press("ControlOrMeta+End");
    await page.keyboard.press("Enter");
    await page.keyboard.type(`# edited in the source by the journey ${SUFFIX}`);

    await page.getByRole("button", { name: "Check" }).click();
    await expect(page.getByText(/Saves as version/)).toBeVisible({ timeout: 60_000 });
    await guideShot(page, "model-2-checked");

    const saved = page.waitForRequest(
      (request) => request.method() === "PUT" && /\/datamodels\/[^/]+\/source/.test(request.url()) && !request.url().includes("dryRun"),
    );
    await page.getByRole("button", { name: /^Propose v\d/ }).click();
    const body = (await saved).postData() ?? "";
    for (const part of ["Station", "Sensor", "QualityBand", "multivalued: true", `# edited in the source by the journey ${SUFFIX}`]) {
      expect(body, `the saved source carries ${part}`).toContain(part);
    }
    const change = await proposedChange(page);
    await guideShot(page, "model-3-proposed");
    await reject(approver.page, PROJECT, change);
  } finally {
    await sweepDrafts(steward.context, page, PROJECT, MINE, [{ kind: "DataModel", name: MODEL }]);
  }
});

test("a steward drafts a model from a CSV sample into the editor", async ({ browser }) => {
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/models?new=file&lang=en`);
  try {
    const csv = "station,pm10,pm25,observed\nkallio,12.5,7.1,2026-09-25T08:00:00Z\nvallila,9.8,5.2,2026-09-25T08:00:00Z\n";
    await page.getByLabel("Choose a sample file").setInputFiles({ name: `${SAMPLE}.csv`, mimeType: "text/csv", buffer: Buffer.from(csv) });
    const draft = page.getByRole("dialog", { name: `Draft from ${SAMPLE}.csv` });
    await expect(draft).toBeVisible({ timeout: 120_000 });
    await expect(draft).toContainText("pm10");
    await draft.getByRole("button", { name: "Populate the editor" }).click();

    // The editor holds the drafted model under the file's name, still to be placed in a space.
    const region = page.getByRole("region", { name: "New model" });
    await expect(region.getByLabel("Name", { exact: true })).toHaveValue(SAMPLE, { timeout: 30_000 });
    await page.getByRole("tab", { name: "Source" }).click();
    await expect(page.locator(".monaco-editor .view-lines").first()).toContainText("pm10", { timeout: 60_000 });
  } finally {
    await sweepDrafts(context, page, PROJECT, MINE);
  }
});

test("a steward drafts a model from what a public API answers, and an address with a key is refused (T-3250)", async ({ browser }) => {
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/models?new=file&lang=en`);
  try {
    const address = page.getByLabel("Or the address of an API that answers JSON");
    // Refused in the browser and never sent: the runner and its logs would hold the key.
    await address.fill("https://api.citybik.es/v2/networks/citybikes-helsinki?api_key=not-a-real-key");
    await page.getByRole("button", { name: "Read the address" }).click();
    await expect(page.getByRole("alert")).toContainText("carries a user name, a password or a key");

    // Fetched once on the project's runner; the model is drawn from the stations inside the answer.
    await address.fill("https://api.citybik.es/v2/networks/citybikes-helsinki");
    await page.getByRole("button", { name: "Read the address" }).click();
    const draft = page.getByRole("dialog", { name: /^Draft from citybikes-helsinki/ });
    await expect(draft).toBeVisible({ timeout: 120_000 });
    await expect(draft).toContainText(/free_?bikes/i);
    await draft.getByRole("button", { name: "Cancel" }).click();
  } finally {
    await sweepDrafts(context, page, PROJECT, MINE);
  }
});

test("a steward imports a Smart Data Model into a model being built", async ({ browser }) => {
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/models?lang=en`);
  try {
    await newModel(page);
    await addClass(page, "Station");

    await page.getByRole("button", { name: "Import a Smart Data Model" }).first().click();
    await page.getByRole("searchbox", { name: "Search models and attributes" }).fill("AirQualityObserved");
    await page.getByRole("button", { name: /^AirQualityObserved/ }).first().click({ timeout: 120_000 });
    await page.getByRole("button", { name: "Import AirQualityObserved" }).click({ timeout: 120_000 });

    // Merged into the model in hand: its own class kept, the upstream one added beside it.
    await expect(page.getByRole("button", { name: "AirQualityObserved", exact: true })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Station", exact: true })).toBeVisible();
  } finally {
    await sweepDrafts(context, page, PROJECT, MINE, [{ kind: "DataModel", name: MODEL }]);
  }
});

test("a steward proposes removing a model, and the rejected removal keeps it", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/datamodels?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  const listed = await page.request.get(`/api/v1/projects/${PROJECT}/datamodels`);
  expect(listed.ok()).toBe(true);
  const names = ((await listed.json()) as { items: { metadata: { name: string } }[] }).items.map((item) => item.metadata.name);
  const dialog = page.getByRole("dialog");
  const propose = dialog.getByRole("button", { name: "Propose removal" });
  const id = dialog.getByText(/^chg-[0-9a-f]{8}$/);
  // A model still used by a space is refused before anything is typed (409, named), and one whose
  // removal an earlier run left open is refused at Propose: take the first one that proposes.
  let model: string | undefined;
  for (const candidate of names) {
    // The model's own page holds Remove since the T-4829 list (T-5066).
    await page.goto(`/projects/${PROJECT}/models/${candidate}?lang=en`);
    const dryRun = page.waitForResponse((answer) => answer.request().method() === "DELETE" && answer.url().includes("dryRun"));
    await page.getByRole("button", { name: /^Remove / }).click();
    if ((await dryRun).status() === 409) {
      await expect(dialog.getByText("Still in use")).toBeVisible();
      await dialog.getByRole("button", { name: "Cancel" }).click();
      continue;
    }
    // Nothing is proposed until the name is typed back (a Red change, CC-19).
    await expect(propose).toHaveAttribute("aria-disabled", "true");
    await dialog.getByLabel(`Type ${candidate} to confirm`).fill(candidate);
    await propose.click();
    const open = dialog.getByRole("alert").filter({ hasText: /is already open: chg-/ });
    await expect(id.or(open)).toBeVisible({ timeout: 60_000 });
    if (await id.isVisible()) {
      model = candidate;
      break;
    }
    await dialog.getByRole("button", { name: "Cancel" }).click();
  }
  expect(model, "every model the project has is still in use or has a removal open").toBeDefined();
  await reject(approver.page, PROJECT, (await id.textContent()) ?? "");

  const kept = await page.request.get(`/api/v1/projects/${PROJECT}/datamodels/${model}`);
  expect(kept.status(), "a rejected removal removed the model").toBe(200);
});

test("a viewer meets the model buttons refused with the reason, and the source route refuses a save", async ({ browser }) => {
  const { page, context } = await signIn(browser, VIEWER, `/projects/${PROJECT}/models?lang=en`);
  const create = page.getByRole("button", { name: "New data model" }).first();
  await expect(create).toHaveAttribute("aria-disabled", "true", { timeout: 60_000 });
  await expect(create).toHaveAccessibleDescription(/does not permit 'propose' on 'DataModel'/);

  // A save of a model the project has: the route checks the role before it reads the source.
  const listed = await page.request.get(`/api/v1/projects/${PROJECT}/datamodels`);
  expect(listed.ok()).toBe(true);
  const [model] = ((await listed.json()) as { items: { metadata: { name: string } }[] }).items.map((item) => item.metadata.name);
  expect(model, "the project has no model").toBeDefined();
  const answer = await page.request.put(`/api/v1/projects/${PROJECT}/datamodels/${model}/source`, {
    headers: { "x-csrf-token": await csrf(context), "content-type": "text/yaml" },
    data: `id: https://example.org/${model}\nname: ${model}\nclasses:\n  Station: {}\n`,
  });
  expect(answer.status(), await answer.text()).toBe(403);
});

test("a viewer reads a model on the full-screen canvas, lights an Endpoint's classes and may not edit (T-6094)", async ({ browser }) => {
  const { page, context } = await signIn(browser, VIEWER, `/projects/${PROJECT}/models?lang=en`);
  const listed = await page.request.get(`/api/v1/projects/${PROJECT}/datamodels`);
  expect(listed.ok()).toBe(true);
  const name = ((await listed.json()) as { items: { metadata: { name: string } }[] }).items[0]?.metadata.name;
  expect(name, "the project has a model").toBeDefined();
  await page.goto(`/projects/${PROJECT}/models/${name}/diagram?lang=en`);
  await expect(page.getByRole("heading", { level: 1, name: `Diagram of ${name}` })).toBeVisible();
  const overlay = page.getByLabel("Show what serves the classes");
  const endpoints = await overlay.locator('option[value^="endpoint:"]').evaluateAll((options) => options.map((one) => (one as HTMLOptionElement).value));
  if (endpoints.length > 0) {
    await overlay.selectOption(endpoints[0]);
    await expect(page.getByText(/of \d+ (is|are) reached; the rest is faded\./)).toBeVisible();
  }
  await page.getByRole("button", { name: / in the structure view$/ }).first().click();
  const panel = page.getByRole("complementary");
  await expect(panel.getByRole("table", { name: /^Fields of / })).toBeVisible();
  await expect(panel.getByRole("button", { name: /Edit in the form/ })).toHaveAttribute("aria-disabled", "true");
  await context.close();
});
