/**
 * Ready for the demo (T-2746, TS-01): the demo of DEMO.md walked on dev in its order, as the
 * people it is for — demo.steward, demo.approver, demo.viewer and a person the walk creates. Each
 * step is one test and asserts what a person sees; the hourly sweep (`sweep.sh`) turns a red one
 * into a task with its screenshot. The steps run serially: a later step stands on what an earlier
 * one made, so the first red one is the one to fix.
 *
 *  1. The themed sign-in, the Organization link and the profile block.
 *  2. Organization: settings, a person created, a group with them in it given a role, and the
 *     person's page listing both; the viewer refused New person.
 *  3. A space with its one model: two classes, an enum, a slot of it, a relationship.
 *  4. A data source, a pipeline through the workbench's steps into helsinki, data in the grid.
 *  5. Endpoints on helsinki, one read and one read+update, and a query through the first.
 *  6. The seeded role-scoped app inside the Portal and in a window of its own, a login in front
 *     of it, the viewer (a member of its default group) let in and the approver refused. Building
 *     an app from the assistant's path spends the model, so it is the nightly
 *     `readiness-app.spec.ts`, not this hourly walk.
 *  7. A KPI read off live data in the pipeline studio.
 *  8. CKAN: the read endpoint published as a dataset, its page read without a login.
 *  9. Every path of the assistant's dock answers its first question (no model call).
 * 10. Everything the walk made is removed, the person signed out everywhere and deleted, and the
 *     steward signs out.
 *
 * Names end in the run's HHMM so the janitor (T-2627) sweeps what a dead run leaves; `afterAll`
 * removes it too when a red step stopped the walk before step 10.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Locator, Page } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, checkManifest, csrf, goSignedIn, portalReady, proposedChange, proposeDelete, reject, removeCompletely, signIn, sweepDrafts } from "./portal";
import { proposeFrom } from "./kindJourney";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";
const PERSON = { email: `readiness-${SUFFIX}@example.org`, first: "Ready", last: `Walk ${SUFFIX}`, edited: `Walked ${SUFFIX}` };
/** The person's name once step 2 has edited it. */
const EDITED = `${PERSON.first} ${PERSON.edited}`;
const GROUP = `rdy-group-${SUFFIX}`;
const SPACE = `rdy-space-${SUFFIX}`;
const MODEL = `rdy-model-${SUFFIX}`;
const SOURCE = `rdy-source-${SUFFIX}`;
const PIPELINE = `rdy-pipeline-${SUFFIX}`;
const READ = `rdy-read-${SUFFIX}`;
const WRITE = `rdy-write-${SUFFIX}`;
const KPI = `rdy-kpi-${SUFFIX}`;
const ACCOUNT = `rdy-sa-${SUFFIX}`;
const SYNC = `rdy-sync-${SUFFIX}`;
const FLOW = `rdy-flow-${SUFFIX}`;
const POLICY = `rdy-policy-${SUFFIX}`;
const MINE = new RegExp(`^rdy-[a-z]+-${SUFFIX}$`);
const ROLE = "model-editor";
const APP = "helsinki-alerts";
const STATIONS = "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_information.json";
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";
/** The seeded stations pipeline's mapping, first 20 stations: the same ids and values, no residue. */
const MAPPING = [
  'let domain = env("JC_ORG_DOMAIN")',
  "root = this.data.stations.slice(0, 20).map_each(s -> {",
  '  "id": "urn:ngsi-ld:BikeHireDockingStation:%v:helsinki:%v".format($domain, s.station_id),',
  '  "type": "BikeHireDockingStation",',
  '  "name": { "type": "LanguageProperty", "languageMap": { "fi": s.name.string() } },',
  '  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [ s.lon.number(), s.lat.number() ] } },',
  '  "totalSlotNumber": { "type": "Property", "value": s.capacity.number() },',
  '  "source": { "type": "Property", "value": "https://www.hsl.fi/en/citybikes" }',
  "})",
].join("\n");
/** Each path of the dock and the question it opens with (`src/agents/paths.rs`). */
const PATHS: [string, string][] = [
  ["Integrate a pipeline", "Where does the data come from?"],
  ["Upload data", "Which space should the data go into?"],
  ["Find data", "What are you looking for?"],
  ["Share data", "Which data do you want to share?"],
  ["Build an app", "Which endpoints should the app read?"],
  ["Build a dashboard", "Which endpoint should the dashboard draw?"],
  ["Create a data model", "Where does the model start?"],
  ["Define a KPI", "Which space do you measure?"],
];

let steward: Session;
let approver: Session;
let viewer: Session;
let personId = "";
let cleaned = false;

test.describe.configure({ mode: "serial" });
// The realm on dev sends no e-mail, so a new person's and a reset password is shown on the page
// once; a trace would keep that page (T-2746 Security). Screenshots are taken only with no
// password dialog open (`afterEach`).
test.use({ trace: "off" });

/** Approves a change as `who`, typing the resource's name back when the page asks for it (CC-19). */
async function approveAsked(who: Session, project: string, change: string): Promise<void> {
  const { page } = who;
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const typed = page.locator("#confirm-resource-name");
  const asked = await typed
    .waitFor({ state: "visible", timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
  await approve(page, project, change, asked ? ((await typed.getAttribute("placeholder")) ?? "") : undefined);
}

async function status(page: Page, route: string): Promise<number> {
  return (await page.request.get(route)).status();
}

async function items<T>(page: Page, route: string): Promise<T[]> {
  const answer = await page.request.get(route);
  expect(answer.ok(), `${route}: ${answer.status()}`).toBe(true);
  return ((await answer.json()) as { items?: T[] }).items ?? [];
}

/** The organization's bindings that name the walk's group. */
async function bindingsOfGroup(page: Page): Promise<string[]> {
  const bindings = await items<{ metadata: { name: string }; spec: { subjects?: { group?: string }[] } }>(
    page,
    "/api/v1/projects/org/rolebindings",
  );
  return bindings
    .filter((binding) => (binding.spec.subjects ?? []).some((subject) => subject.group === GROUP))
    .map((binding) => binding.metadata.name);
}

async function slugOf(page: Page, endpoint: string): Promise<string> {
  const endpoints = await items<{ metadata: { name: string }; spec?: { slug?: string } }>(
    page,
    `/api/v1/projects/${PROJECT}/endpoints`,
  );
  return endpoints.find((one) => one.metadata.name === endpoint)?.spec?.slug ?? "";
}

/** Waits until the project's API lists `name` under `plural`, then reloads the page. */
async function listed(page: Page, project: string, plural: string, name: string): Promise<void> {
  await expect
    .poll(async () => status(page, `/api/v1/projects/${project}/${plural}/${name}`), {
      timeout: 300_000,
      intervals: [5_000],
      message: `${plural}/${name} reaches the mirror`,
    })
    .toBe(200);
  await page.reload({ waitUntil: "load" });
}

/** The endpoint form of a new endpoint on helsinki, public, its class ticked, proposed. */
async function newEndpoint(page: Page, name: string, writable: boolean): Promise<string> {
  await page.goto(`/projects/${PROJECT}/endpoints?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "New endpoint" }).first().click();
  const form = page.getByTestId("form-page");
  await form.locator("#root_name").fill(name);
  await form.locator("#root_title").fill(`Readiness ${name}`);
  await form.locator("#root_contextSpaceRef").selectOption(PROJECT);
  // Public: a signed-in person holds the role `public`, so the steward reads it (gateway app.rs).
  await form.locator("#root_audience").selectOption({ label: "Public: anyone with the address" });
  await form.getByRole("checkbox", { name: "BikeHireDockingStation", exact: true }).check();
  if (writable) {
    await form.getByRole("checkbox", { name: "BikeHireDockingStation Writable" }).check();
  }
  await proposeFrom(form);
  return proposedChange(page);
}

/**
 * Removes everything the walk made, dependents first, so no removal meets a reference (409), and
 * waits until the mirror forgets the group, so the person's removal is immediate (204). The
 * person goes too unless `keepPerson`: step 10 removes them through their page.
 */
async function cleanUp(keepPerson = false): Promise<void> {
  const page = steward.page;
  await removeCompletely(steward, PROJECT, "pipelines", PIPELINE);
  await removeCompletely(steward, PROJECT, "syncsources", SYNC);
  await removeCompletely(steward, PROJECT, "serviceaccounts", ACCOUNT);
  await removeCompletely(steward, PROJECT, "datasources", SOURCE);
  for (const endpoint of [READ, WRITE]) {
    await removeCompletely(steward, PROJECT, "endpoints", endpoint);
  }
  const projections = await items<{ metadata: { name: string } }>(page, `/api/v1/projects/${PROJECT}/projections`);
  for (const { metadata } of projections.filter((one) => one.metadata.name.startsWith(READ) || one.metadata.name.startsWith(WRITE))) {
    await removeCompletely(steward, PROJECT, "projections", metadata.name);
  }
  const policies = await items<{ metadata: { name: string } }>(page, `/api/v1/projects/${PROJECT}/policies`);
  for (const { metadata } of policies.filter((one) => one.metadata.name.startsWith(READ) || one.metadata.name.startsWith(WRITE))) {
    await removeCompletely(steward, PROJECT, "policies", metadata.name);
  }
  await removeCompletely(steward, PROJECT, "policies", POLICY);
  await removeCompletely(steward, PROJECT, "spaces", SPACE);
  await removeCompletely(steward, PROJECT, "datamodels", MODEL);
  await sweepDrafts(steward.context, page, PROJECT, MINE);

  for (const binding of await bindingsOfGroup(page)) {
    await removeCompletely(steward, "org", "rolebindings", binding);
  }
  await removeCompletely(steward, "org", "groups", GROUP);
  await sweepDrafts(steward.context, page, "org", MINE);
  await expect
    .poll(() => status(page, `/api/v1/projects/org/groups/${GROUP}`), { timeout: 300_000, intervals: [5_000] })
    .toBe(404);
  if (personId && !keepPerson) {
    const gone = await page.request.delete(`/api/v1/organization/people/${personId}`, {
      headers: { "x-csrf-token": await csrf(steward.context) },
    });
    expect([204, 404], await gone.text()).toContain(gone.status());
  }
}

test.beforeAll(async ({ browser }) => {
  test.setTimeout(300_000);
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/spaces?lang=en`);
});

test.afterAll(async () => {
  test.setTimeout(1_200_000);
  try {
    if (!cleaned && steward) {
      await cleanUp();
    }
  } finally {
    await steward?.context.close();
    await approver?.context.close();
    await viewer?.context.close();
  }
});

test.afterEach(async () => {
  const info = test.info();
  if (info.status !== info.expectedStatus && steward) {
    if (await steward.page.getByRole("dialog", { name: "Temporary password" }).count()) {
      info.annotations.push({ type: "screenshot", description: "withheld: a temporary password was on the page" });
      return;
    }
    // The sweep files the step with this picture of what the person saw (sweep-summary.ts).
    await info.attach("screenshot", { body: await steward.page.screenshot({ fullPage: true }), contentType: "image/png" });
  }
});

test("1. the themed sign-in leads to the Portal with the Organization link and the profile block", async ({ browser }) => {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
    const signInButton = page.getByRole("button", { name: "Sign in" });
    await expect(page.locator("#username").or(signInButton).first()).toBeVisible({ timeout: 60_000 });
    if (!(await page.locator("#username").count())) {
      await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), signInButton.first().click()]);
    }
    // The platform's own theme, not Keycloak's default: its header carries the logo and the org.
    await expect(page.locator('body[data-page-id^="login-"]')).toHaveCount(1, { timeout: 60_000 });
    await expect(page.locator("#kc-header img.jc-logo")).toBeVisible();
    await expect(page.locator("#kc-header .jc-org")).not.toBeEmpty();
    await goSignedIn(page, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);

    const nav = portalReady(page);
    // Sign-out sits in the header's account menu only since owner 2026-11-01.
    await expect(nav.getByRole("region", { name: "Your account" })).toHaveCount(0);
    await expect(page.getByRole("link", { name: "Organization", exact: true }).first()).toBeVisible();
  } finally {
    await context.close();
  }
});

test("2. the organization: a settings edit rejected, a new person edited, disabled, enabled and reset, a group with a role, and the person's page", async () => {
  test.setTimeout(1_200_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("link", { name: "Organization", exact: true }).first().click();
  await expect(page).toHaveURL(/\/organization\/settings/);
  await expect(page.getByRole("heading", { level: 1, name: "Organization" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 2, name: "Settings" })).toBeVisible();
  await expect(page.getByText("Domain", { exact: true }).first()).toBeVisible();

  // An edit of the settings, proposed as a red change, and rejected by the administrator with a
  // reason, so the organization stays as it was.
  await page.getByRole("button", { name: /^Edit [a-z0-9.-]+\.[a-z]+$/ }).first().click();
  const settings = page.getByTestId("form-page");
  const apps = settings.getByLabel("Apps", { exact: true });
  const quota = Number.parseInt((await apps.inputValue()) || "0", 10);
  await apps.fill(String(quota + 1));
  await proposeFrom(settings);
  const edit = await proposedChange(page);
  await reject(page, "org", edit);
  await expect
    .poll(async () => {
      const answer = await page.request.get(`/api/v1/projects/org/changes/${edit}`);
      return answer.ok() ? (((await answer.json()) as { status?: { phase?: string } }).status?.phase ?? "") : "";
    }, { timeout: 60_000 })
    .toBe("Rejected");
  await page.goto("/organization/settings?lang=en", { waitUntil: "load" });

  // A person, created from the People tab; the realm on dev sends no e-mail, so the page hands
  // over a temporary password once, which the walk closes unread.
  await page.getByRole("tab", { name: "People" }).click();
  await page.getByRole("button", { name: "New person" }).click();
  const form = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
  await form.getByLabel(/^E-mail/).fill(PERSON.email);
  await form.getByLabel(/^First name/).fill(PERSON.first);
  await form.getByLabel(/^Last name/).fill(PERSON.last);
  const created = page.waitForResponse(
    (response) => response.url().endsWith("/api/v1/organization/people") && response.request().method() === "POST",
  );
  await form.getByRole("button", { name: "Create person" }).click();
  const answer = await created;
  expect(answer.status(), "the person is created").toBe(201);
  personId = ((await answer.json()) as { person: { id: string } }).person.id;
  const handOver = page.getByRole("dialog", { name: "Temporary password" });
  const sent = page.getByText(`The realm sent ${PERSON.email}`);
  await expect(handOver.or(sent).first()).toBeVisible({ timeout: 60_000 });
  if (await handOver.count()) {
    await handOver.getByRole("button", { name: "Done" }).click();
  }
  await page.goto("/organization/people?lang=en", { waitUntil: "load" });
  await page.getByRole("searchbox", { name: "Search people" }).fill(PERSON.email);
  await page.getByRole("button", { name: "Search" }).click();
  await expect(page.getByRole("link", { name: `${PERSON.first} ${PERSON.last}` })).toBeVisible({ timeout: 60_000 });

  // The person's page: edit, disable, enable, reset the password, remove a second factor. Each
  // goes straight to the realm; the walk acts only on the person it created.
  await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const editor = page.getByRole("dialog", { name: `Edit ${PERSON.first} ${PERSON.last}` });
  await editor.getByLabel(/^Last name/).fill(PERSON.edited);
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("heading", { level: 1, name: EDITED })).toBeVisible({ timeout: 60_000 });
  const confirmed = async (button: string, said: string | RegExp) => {
    await page.getByRole("button", { name: button, exact: true }).click();
    await page.getByTestId("confirm-accept").click();
    await expect(page.getByText(said).first()).toBeVisible({ timeout: 60_000 });
  };
  await confirmed("Disable", "Disabled. Every session of the person ended.");
  await expect(page.getByRole("button", { name: "Enable", exact: true })).toBeVisible();
  await confirmed("Enable", "Enabled. The person can sign in again.");
  await page.getByRole("button", { name: "Reset password", exact: true }).click();
  await page.getByTestId("confirm-accept").click();
  const reset = page.getByRole("dialog", { name: "Temporary password" });
  await expect(reset.or(page.getByText("The realm e-mailed the person a link to set a new password.")).first()).toBeVisible({
    timeout: 60_000,
  });
  if (await reset.count()) {
    // Closed unread: the walk proves the hand-over, never the password.
    await reset.getByRole("button", { name: "Done" }).click();
  }
  await confirmed("Remove second factor", "Second factor removed.");

  // The viewer meets New person refused, with the reason.
  await viewer.page.goto("/organization/people?lang=en", { waitUntil: "load" });
  const refused = viewer.page.getByRole("button", { name: "New person" });
  await expect(refused).toHaveAttribute("aria-disabled", "true", { timeout: 60_000 });
  await expect(refused).toHaveAttribute("title", /permit 'create' on 'Person'/);

  // A group with the person in it: a red change the administrator approves (PF-58).
  await page.goto("/organization/groups/new?lang=en", { waitUntil: "load" });
  const group = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
  await group.locator("#root_name").fill(GROUP);
  await group.locator("#root_description").fill("The readiness walk's group");
  await group.getByRole("button", { name: "Add" }).first().click();
  await group.locator("#root_members_0_user").fill(PERSON.email);
  await proposeFrom(group);
  await approveAsked(steward, "org", await proposedChange(page));
  await listed(page, "org", "groups", GROUP);

  // The group gets a role at organization scope, on its own page.
  await page.goto(`/organization/groups/${GROUP}?lang=en`, { waitUntil: "load" });
  const roles = page.getByRole("region", { name: "Platform roles" });
  await roles.getByLabel("Role", { exact: true }).selectOption({ label: ROLE });
  await roles.getByRole("button", { name: "Give role" }).click();
  await approveAsked(steward, "org", await proposedChange(page));
  await expect.poll(() => bindingsOfGroup(page), { timeout: 300_000, intervals: [5_000] }).toHaveLength(1);

  // The person's page lists the group and the role that reaches them through it.
  await expect
    .poll(
      async () => {
        await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
        const groups = page.getByRole("region", { name: "Groups" }).getByRole("link", { name: GROUP });
        const role = page.getByRole("region", { name: "Platform roles" }).getByText(ROLE).first();
        return (await groups.count()) > 0 && (await role.count()) > 0;
      },
      { timeout: 300_000, intervals: [10_000], message: "the person's page shows the group and its role" },
    )
    .toBe(true);
  await expect(page.getByRole("heading", { level: 1, name: EDITED })).toBeVisible();
});

test("3. a space with its model: two classes, an enum, a slot of it and a relationship", async () => {
  test.setTimeout(900_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: /^New (context )?space/i }).first().click();
  const space = page.getByTestId("form-page");
  await space.getByLabel(/^Name/).fill(SPACE);
  await proposeFrom(space);
  await approveAsked(approver, PROJECT, await proposedChange(page));
  await listed(page, PROJECT, "spaces", SPACE);

  await page.goto(`/projects/${PROJECT}/models?new=blank&space=${SPACE}&lang=en`, { waitUntil: "load" });
  const editor = page.getByRole("region", { name: "New model" });
  await editor.getByLabel("Name", { exact: true }).fill(MODEL);
  await editor.getByLabel("Space", { exact: true }).selectOption(SPACE);
  for (const name of ["Station", "Sensor"]) {
    await page.getByLabel("New type").fill(name);
    await page.getByRole("button", { name: "Add type" }).click();
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  }
  await page.getByLabel("New enum").fill("QualityBand");
  await page.getByRole("button", { name: "Add enum" }).click();
  for (const value of ["good", "poor"]) {
    await page.getByLabel("New value of QualityBand").fill(value);
    await page.getByRole("button", { name: "Add value to QualityBand" }).click();
  }
  await page.getByRole("button", { name: "Station", exact: true }).click();
  await page.getByLabel("New field").fill("band");
  await page.getByRole("button", { name: "Add field" }).click();
  await page.getByLabel("Range", { exact: true }).selectOption("QualityBand");
  await page.getByRole("radio", { name: "One to many" }).check();
  await page.getByLabel("Target class").selectOption("Sensor");
  await page.getByRole("button", { name: "Add relationship" }).click();
  await expect(page.getByRole("table", { name: "Relationships" })).toContainText("has many");
  await page.getByRole("button", { name: "Check" }).click();
  await expect(page.getByText(/Saves as version/)).toBeVisible({ timeout: 60_000 });
  await page.getByRole("button", { name: /^Propose v\d/ }).click();
  await approveAsked(approver, PROJECT, await proposedChange(page));
  await listed(page, PROJECT, "datamodels", MODEL);
  await expect(page.getByText(MODEL).first()).toBeVisible();
});

test("4. a data source and a pipeline through the workbench's steps, and its data in the grid", async () => {
  test.setTimeout(1_200_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/datasources?lang=en`, { waitUntil: "load" });
  await page.getByLabel("Type").selectOption("http");
  await page.getByRole("button", { name: "New data source" }).click();
  const source = page.getByTestId("form-page");
  await source.getByLabel(/^Name/).fill(SOURCE);
  await source.getByLabel(/^URL/).fill(STATIONS);
  await source.getByLabel(/^Timeout/).fill("15s");
  await source.getByRole("button", { name: "Check" }).click();
  await expect(source.getByText("Planned change")).toBeVisible({ timeout: 60_000 });
  await source.getByRole("button", { name: "Propose change" }).click();
  await approveAsked(approver, PROJECT, await proposedChange(page));
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
  await listed(page, PROJECT, "datasources", SOURCE);

  await page.getByRole("button", { name: "New pipeline" }).first().click();
  const form = page.getByTestId("form-page");
  const step = (title: string): Locator => form.getByRole("region", { name: new RegExp(`${title}$`) });
  await form.locator("#workbench-source-pick").selectOption(`datasource:${SOURCE}`);
  await expect(step("Sample").getByRole("table", { name: "Sample records" })).toBeVisible({ timeout: 90_000 });
  await form.locator("#root_name").fill(PIPELINE);
  await form.getByText("More options").click();
  await form.locator("#root_period").fill("60s");
  await form.locator("#workbench-target-pick").selectOption(TARGET);
  await form.locator("#workbench-bloblang").fill(MAPPING);
  await expect(step("Mapped output").getByRole("table", { name: "Mapped records" })).toBeVisible({ timeout: 90_000 });
  await expect(step("Validation").getByText(/All \d+ records are valid against helsinki/)).toBeVisible({ timeout: 90_000 });
  await expect(
    step("Target").getByText("The records land in the space helsinki, checked against the model helsinki."),
  ).toBeVisible();
  await proposeFrom(form);
  await approveAsked(approver, PROJECT, await proposedChange(page));

  // One run wrote what it read.
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${PIPELINE}/runs`);
        const runs = answer.ok() ? (((await answer.json()) as { items?: { sent: number }[] }).items ?? []) : [];
        return runs.some((run) => run.sent > 0);
      },
      { timeout: 420_000, intervals: [10_000], message: "the pipeline ran and wrote" },
    )
    .toBe(true);

  // The grid: Explore on helsinki's stations shows rows.
  await expect
    .poll(
      async () => {
        await page.goto(`/projects/${PROJECT}/explore?lang=en`, { waitUntil: "load" });
        await page.locator("#explore-space").selectOption(PROJECT);
        await page.locator("#explore-endpoint").selectOption("helsinki-all");
        const kind = page.locator("#explore-type");
        if ((await kind.evaluate((element) => element.tagName)) === "SELECT") {
          await kind.selectOption("BikeHireDockingStation");
        } else {
          await kind.fill("BikeHireDockingStation");
          await kind.press("Enter");
        }
        const count = await page
          .getByText(/^\d+ entit(y|ies)$/)
          .first()
          .textContent({ timeout: 15_000 })
          .catch(() => "0 entities");
        return Number.parseInt(count ?? "0", 10);
      },
      { timeout: 240_000, intervals: [10_000], message: "the grid shows the stations" },
    )
    .toBeGreaterThan(0);
});

test("5. an endpoint that reads and one that reads and updates, and a query through the first", async () => {
  test.setTimeout(1_200_000);
  const page = steward.page;
  await approveAsked(approver, PROJECT, await newEndpoint(page, READ, false));
  await approveAsked(approver, PROJECT, await newEndpoint(page, WRITE, true));
  for (const endpoint of [READ, WRITE]) {
    await expect
      .poll(
        async () => {
          await page.goto(`/projects/${PROJECT}/endpoints?lang=en`, { waitUntil: "load" });
          return page.getByRole("row").filter({ hasText: endpoint }).getByText("Live").count();
        },
        { timeout: 300_000, intervals: [10_000], message: `${endpoint} is live` },
      )
      .toBeGreaterThan(0);
  }

  // What each grants, as the approved policies say.
  const operations = async (name: string): Promise<string[]> => {
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/policies/${name}`);
    return answer.ok() ? (((await answer.json()) as { spec?: { operations?: string[] } }).spec?.operations ?? []) : [];
  };
  expect(await operations(`${READ}-read`)).toEqual(["retrieveOps"]);
  expect(await status(page, `/api/v1/projects/${PROJECT}/policies/${READ}-write`), "the read endpoint grants no write").toBe(404);
  expect(await operations(`${WRITE}-read`)).toEqual(["retrieveOps"]);
  expect(await operations(`${WRITE}-write`)).toEqual(["updateOps"]);

  // The query, through the read endpoint.
  const slug = await slugOf(page, READ);
  expect(slug, `${READ} has a slug`).not.toBe("");
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=BikeHireDockingStation&limit=10`);
        return answer.ok() ? ((await answer.json()) as unknown[]).length : -answer.status();
      },
      { timeout: 180_000, intervals: [10_000], message: "the read endpoint answers stations" },
    )
    .toBeGreaterThan(0);
});

test("6. an app opens inside the Portal and in its own window behind a login; its group's member gets in, others do not", async ({ browser }) => {
  test.setTimeout(900_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/apps/${APP}/open?lang=en`, { waitUntil: "load" });
  await expect(page.locator("iframe[sandbox]")).toHaveAttribute("sandbox", "allow-scripts allow-forms allow-popups allow-downloads");
  await expect(
    page.frameLocator("iframe[sandbox]").getByRole("region", { name: "Overview" }).getByText(/^[a-zA-Z][\w -]*: \d+$/).first(),
  ).toBeVisible({ timeout: 120_000 });
  const popup = page.waitForEvent("popup");
  await page.getByRole("link", { name: /Open in new window/ }).click();
  const own = await popup;
  await own.waitForURL((url) => url.pathname === `/apps/${APP}/`, { timeout: 60_000 });
  await own.close();

  // Nobody signed in meets the login in front of the app.
  const anonymous = await browser.newContext();
  try {
    const stranger = await anonymous.newPage();
    await stranger.goto(`${APPS_URL}/apps/${APP}/`, { waitUntil: "load" });
    await expect(stranger.locator("#username")).toBeVisible({ timeout: 60_000 });
  } finally {
    await anonymous.close();
  }

  // The viewer is in the app's default viewer group and gets in; the approver is in none.
  await goSignedIn(viewer.page, VIEWER, `${APPS_URL}/apps/${APP}/`, (opened) => opened.getByRole("region", { name: "Overview" }));
  await goSignedIn(approver.page, APPROVER, `${APPS_URL}/apps/${APP}/`, (opened) =>
    opened.getByText("Only a person holding one of these roles can open this application:"),
  );
});

test("7. a KPI reads a value off live data", async () => {
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "New pipeline" }).first().click();
  const studio = page.getByTestId("form-page");
  await studio.locator("#studio-preset").selectOption("kpi");
  await studio.locator("#studio-kpi-endpoint").selectOption("helsinki-all");
  await studio.locator("#studio-kpi-name").fill(KPI);
  await studio.getByTestId("studio-kpi-test").click();
  await expect(studio.getByTestId("studio-kpi-value")).toHaveText(/\d/, { timeout: 60_000 });
});

test("8. CKAN: the read endpoint is published as a dataset whose page anyone reads", async ({ browser }) => {
  test.setTimeout(900_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/endpoints/${READ}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Publish a dataset" }).click();
  const dialog = page.getByRole("dialog", { name: "Publish a dataset" });
  await dialog.getByRole("button", { name: "Draft the description" }).click();
  const licence = dialog.locator("#publish-licence");
  await expect(licence).toBeVisible({ timeout: 120_000 });
  if (!(await licence.inputValue())) {
    await licence.selectOption({ index: 1 });
  }
  await dialog.getByRole("button", { name: "Preview the entry" }).click();
  await dialog.getByRole("button", { name: /propose publication$/i }).click();
  await approveAsked(approver, PROJECT, await proposedChange(page));

  const anonymous = await browser.newContext();
  try {
    const reader = await anonymous.newPage();
    await expect
      .poll(
        async () => {
          await reader.goto(`/catalogue/${READ}?lang=en`, { waitUntil: "load" });
          return reader.getByRole("heading", { name: "Resources" }).count();
        },
        { timeout: 420_000, intervals: [15_000], message: "the dataset's page reaches the catalogue" },
      )
      .toBeGreaterThan(0);
    await expect(reader.getByRole("button", { name: "Sign in" })).toBeVisible();
  } finally {
    await anonymous.close();
  }
});

test("9. every path of the dock answers its first question", async () => {
  test.setTimeout(600_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Open the assistant" }).click();
  for (const [path, question] of PATHS) {
    await page.getByTestId("assistant-paths").getByRole("button", { name: new RegExp(`^${path}`) }).click();
    await expect(page.getByTestId("question-options").getByText(question), `${path} asks its first question`).toBeVisible({
      timeout: 30_000,
    });
    await page.getByRole("button", { name: "Start a new conversation" }).click();
    await expect(page.getByTestId("assistant-paths")).toBeVisible({ timeout: 30_000 });
  }
});

test("9a. a service account's API key is minted, rotated and revoked", async () => {
  test.setTimeout(900_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/settings/service-accounts?lang=en`, { waitUntil: "load" });
  await page.getByRole("main").getByRole("button", { name: "New service account" }).first().click();
  const form = page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New service account" })).first();
  await form.locator("#root_name").fill(ACCOUNT);
  await form.locator("#root_purpose").fill("Holds the readiness walk's API key");
  await form.locator("#root_roles_0_role").fill("viewer");
  await form.locator("#root_credentials_0_name").fill(`${ACCOUNT}-key`);
  await form.locator("#root_credentials_0_kind").selectOption("api-key");
  await proposeFrom(form);
  await approveAsked(approver, PROJECT, await proposedChange(page));
  await page.goto(`/projects/${PROJECT}/settings/service-accounts?lang=en`, { waitUntil: "load" });
  await listed(page, PROJECT, "serviceaccounts", ACCOUNT);

  // The key is shown once; the walk checks its shape inside the page and never reads it out.
  const minted = async () => {
    const dialog = page.getByRole("dialog", { name: "Your new API key" });
    await expect(dialog).toBeVisible({ timeout: 60_000 });
    const shaped = await dialog
      .getByLabel("API key")
      .evaluate((input) => /^jc_[A-Za-z0-9]+_[A-Za-z0-9_-]{16,}$/.test((input as HTMLInputElement).value));
    expect(shaped, "the key is a jc_<id>_<secret>").toBe(true);
    await dialog.getByRole("button", { name: "Close" }).click();
  };
  // T-3255: before the key, what it allows and how long it works.
  await page.getByRole("button", { name: `New API key (${ACCOUNT}-key)` }).click();
  const asked = page.getByRole("dialog", { name: `New API key (${ACCOUNT}-key)` });
  await expect(asked.getByText(`viewer in ${PROJECT}`)).toBeVisible();
  await asked.getByRole("radio", { name: /^In 7 days/ }).check();
  await asked.getByRole("button", { name: "Create key" }).click();
  await minted();
  const keys = page.getByRole("table", { name: `API keys of ${ACCOUNT}` });
  await expect(keys.getByRole("row")).toHaveCount(2, { timeout: 30_000 });
  await expect(keys.getByText(/^expires in 7 days$/)).toBeVisible();
  await keys.getByRole("button", { name: "Rotate" }).first().click();
  await minted();
  await expect(keys.getByRole("row")).toHaveCount(3, { timeout: 30_000 });
  // Both keys revoked, so nothing of the account still answers once it is removed.
  for (let left = 2; left > 0; left -= 1) {
    await keys.getByRole("button", { name: "Revoke" }).first().click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Revoke now" }).click();
    await expect(keys.getByRole("button", { name: "Revoke" })).toHaveCount(left - 1, { timeout: 30_000 });
  }
  await expect(keys.getByText(/^revoked /).first()).toBeVisible();
});

test("9b. a sync source syncs, pauses and resumes, refuses an unsigned webhook, and is detached", async () => {
  test.setTimeout(900_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/syncsources?lang=en`, { waitUntil: "load" });
  await page.getByLabel("Origin").selectOption("git");
  await page.getByRole("main").getByRole("button", { name: "Add source" }).click();
  const form = page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New sync source" })).first();
  await form.locator("#root_name").fill(SYNC);
  // example.org (RFC 2606): the loop reaches nothing, so the source never opens a merge request.
  await form.locator("#root_git_url").fill(`https://git.example.org/${SYNC}/config.git`);
  await form.locator("#root_git_ref").fill("main");
  await proposeFrom(form);
  await approveAsked(approver, PROJECT, await proposedChange(page));
  await listed(page, PROJECT, "syncsources", SYNC);

  const card = page.getByRole("article", { name: SYNC });
  await card.getByRole("button", { name: "Sync now" }).click();
  await expect(
    card
      .getByText("The run changed nothing: the source has not moved.")
      .or(card.getByRole("alert"))
      .or(card.getByText("Last error"))
      .first(),
    "Sync now reports what the run did",
  ).toBeVisible({ timeout: 120_000 });
  await card.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(card.getByRole("button", { name: "Sync now" })).toBeDisabled({ timeout: 30_000 });
  await card.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(card.getByRole("button", { name: "Pause", exact: true })).toBeVisible({ timeout: 30_000 });

  // A webhook call is verified against the source's own secret: an unsigned one is refused.
  const unsigned = await page.request.post(`/api/v1/webhooks/sync/${PROJECT}/${SYNC}`, { data: { ref: "refs/heads/main" } });
  expect(unsigned.status(), "an unsigned webhook is refused").toBe(401);

  await card.getByRole("button", { name: "Detach" }).click();
  await page.getByTestId("confirm-accept").click();
  await expect(card.getByText("A merge request that removes the source is open.")).toBeVisible({ timeout: 60_000 });
  let change = "";
  await expect
    .poll(
      async () => {
        const open = await items<{ metadata: { name: string } }>(page, `/api/v1/projects/${PROJECT}/changes`);
        change = open.find((one) => JSON.stringify(one).includes(`detach sync source ${PROJECT}/${SYNC}`))?.metadata.name ?? "";
        return change;
      },
      { timeout: 60_000, message: "the detach is a change to approve" },
    )
    .not.toBe("");
  await approveAsked(steward, PROJECT, change);
  await expect
    .poll(() => status(page, `/api/v1/projects/${PROJECT}/syncsources/${SYNC}`), { timeout: 300_000, intervals: [5_000] })
    .toBe(404);
});

test("9c. a blueprint proposes a flow from its form, and the approver rejects it", async () => {
  test.setTimeout(600_000);
  const page = steward.page;
  // Its card sits on Data sources since the Flows page went; the old address opens the home (T-6096).
  await page.goto(`/projects/${PROJECT}/flows?lang=en`, { waitUntil: "load" });
  await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/home`));
  await page.goto(`/projects/${PROJECT}/datasources?lang=en`, { waitUntil: "load" });
  await page
    .getByRole("listitem")
    .filter({ has: page.getByRole("heading", { name: "Poll a JSON feed into a Context Space" }) })
    .getByRole("button", { name: "Set up" })
    .click();
  await expect(page.getByRole("heading", { name: "Set up: Poll a JSON feed into a Context Space" })).toBeVisible();
  // A fresh name: the defaults could name a seeded pipeline and propose to change it.
  await page.locator("#root_name").fill(FLOW);
  await page.locator("#root_url").fill(`https://feeds.example.org/${FLOW}.json`);
  await page.locator("#root_entityType").fill("Event");
  await page.getByRole("button", { name: "Propose change" }).click();
  const change = await proposedChange(page);
  await reject(approver.page, PROJECT, change);
  await expect
    .poll(async () => {
      const answer = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
      return answer.ok() ? (((await answer.json()) as { status?: { phase?: string } }).status?.phase ?? "") : "";
    }, { timeout: 60_000 })
    .toBe("Rejected");
  expect(await status(page, `/api/v1/projects/${PROJECT}/pipelines/${FLOW}`), "a rejected flow made no pipeline").toBe(404);
});

test("9d. a grant on the walk's space is approved, then removed from the policies page", async () => {
  test.setTimeout(600_000);
  const page = steward.page;
  // No title, so the list's row shows the name the removal is typed back with.
  const policy = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: POLICY, namespace: PROJECT },
    spec: {
      contextSpaceRef: { kind: "ContextSpace", name: SPACE },
      assigner: "did:web:hel.fi",
      assignee: { kind: "serviceAccount", id: "pipelines" },
      operations: ["queryBatch"],
    },
  };
  await checkManifest(page, steward.context, PROJECT, policy);
  const proposed = await page.request.post(`/api/v1/projects/${PROJECT}/policies`, {
    headers: { "x-csrf-token": await csrf(steward.context) },
    data: policy,
  });
  expect(proposed.status(), await proposed.text()).toBe(202);
  await approveAsked(approver, PROJECT, ((await proposed.json()) as { metadata: { name: string } }).metadata.name);
  await listed(page, PROJECT, "policies", POLICY);

  // The administrator's typed name approves the removal as it is proposed (PF-58).
  const removal = await proposeDelete(page, PROJECT, "policies", POLICY);
  expect(removal).toMatch(/^chg-/);
  await expect
    .poll(() => status(page, `/api/v1/projects/${PROJECT}/policies/${POLICY}`), { timeout: 300_000, intervals: [5_000] })
    .toBe(404);
});

test("10. nothing of the walk is left, the person is signed out everywhere and removed, and the steward signs out", async () => {
  test.setTimeout(1_800_000);
  const page = steward.page;
  await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Sign out everywhere" }).click();
  await page.getByTestId("confirm-accept").click();
  await expect(page.getByText("Every session of the person ended.")).toBeVisible({ timeout: 30_000 });

  // The group, removed from its own page: the administrator's typed name approves it at once
  // (PF-58), and the removal takes the group out of the binding that names it.
  await page.goto(`/organization/groups/${GROUP}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: `Remove ${GROUP}`, exact: true }).click();
  const removal = page.getByRole("dialog", { name: `Remove ${GROUP}` });
  await removal.getByLabel(`Type ${GROUP} to confirm`).fill(GROUP);
  await removal.getByRole("button", { name: "Propose removal" }).click();
  await expect(removal.getByText("Approved as you proposed it, applying:")).toBeVisible({ timeout: 60_000 });
  await removal.getByRole("button", { name: "Close" }).click();
  await expect
    .poll(() => status(page, `/api/v1/projects/org/groups/${GROUP}`), { timeout: 300_000, intervals: [5_000] })
    .toBe(404);

  await cleanUp(true);

  // No group names the person any more, so Delete removes them at once.
  await page.goto(`/organization/people/${personId}?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("alertdialog", { name: `Delete ${EDITED}?` })).toBeVisible();
  await page.getByTestId("confirm-accept").click();
  await expect(page).toHaveURL(/\/organization\/people(\?|$)/, { timeout: 60_000 });
  expect(await status(page, `/api/v1/organization/people/${personId}`), "the person is gone").toBe(404);
  cleaned = true;

  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: /^Signed in as / }).click();
  await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), page.getByRole("menuitem", { name: "Sign out" }).click()]);
  await expect.poll(async () => (await page.request.get("/api/v1/auth/me")).ok(), { timeout: 30_000 }).toBe(false);
});
