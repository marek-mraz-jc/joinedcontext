/**
 * Relationships end to end on dev (T-2742; DM-31, DM-64, DM-70, UI-84): a steward builds School,
 * User, Course, Profile and Person with all four cardinalities in the model editor, publishes them
 * to a new space, and writes through an Endpoint of it as a person does. Every refusal is the
 * gateway's own, with its rule, and a viewer's write is refused.
 *
 *  1. The model: School 1:N User (User.school required), User N:M Course, User 1:1 Profile
 *     (cascade), Person N:1 Person (manager, set-null), saved and approved into a new space.
 *  2. An Endpoint over the space that reads and updates every class, and a Policy that lets the
 *     project's writers create them.
 *  3. Entities written through the Endpoint as the steward, and User.school changed in Explore
 *     with the picker, which writes a Relationship.
 *  4. Each violation the gateway holds without the broker, refused with its rule (T-2740 owner
 *     decision: stateless enforcement now): a wrong type, a second school, a missing required end,
 *     a target the space does not hold; and a viewer's write.
 *
 * A taken 1:1 target and the delete rules (restrict, cascade, set-null) need the store's own
 * constraint and come with the broker task (T-2858); this journey grows by those steps then.
 *
 * Names are `t2742-…-HHMM`, so the janitor sweeps what a dead run leaves; `afterAll` removes
 * everything the walk made, dependents first.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, checkManifest, csrf, proposedChange, removeCompletely, signIn, sweepDrafts } from "./portal";
import { proposeFrom } from "./kindJourney";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const ORG_DOMAIN = process.env.E2E_ORG_DOMAIN ?? "hel.fi";
const SPACE = `t2742-space-${SUFFIX}`;
const MODEL = `t2742-model-${SUFFIX}`;
const ENDPOINT = `t2742-endpoint-${SUFFIX}`;
const CREATE = `t2742-create-${SUFFIX}`;
const MINE = new RegExp(`^t2742-[a-z]+-${SUFFIX}$`);
const CLASSES = ["School", "User", "Course", "Profile", "Person"];
/** A space without a pinned segment renders `{project}-{space}` into its ids (PF-10). */
const SEGMENT = `${PROJECT}-${SPACE}`;
const urn = (type: string, local: string) => `urn:ngsi-ld:${type}:${ORG_DOMAIN}:${SEGMENT}:${local}`;

let steward: Session;
let approver: Session;
let viewer: Session;
let slug = "";

test.describe.configure({ mode: "serial" });

/** Approves a change as the approver, typing the resource's name back when the page asks (CC-19). */
async function approveAsked(change: string): Promise<void> {
  const { page } = approver;
  await page.goto(`/projects/${PROJECT}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const typed = page.locator("#confirm-resource-name");
  const asked = await typed
    .waitFor({ state: "visible", timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
  await approve(page, PROJECT, change, asked ? ((await typed.getAttribute("placeholder")) ?? "") : undefined);
}

/** Waits until the project's API lists `name` under `plural`. */
async function listed(page: Page, plural: string, name: string): Promise<void> {
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/${plural}/${name}`)).status(), {
      timeout: 300_000,
      intervals: [5_000],
      message: `${plural}/${name} reaches the mirror`,
    })
    .toBe(200);
}

/** Adds a relationship from the class the editor has open, both ends named. */
async function relate(
  page: Page,
  from: string,
  relation: { cardinality: string; to: string; name: string; inverse: string; requiredOn?: string; onDelete?: string },
): Promise<void> {
  await page.getByRole("button", { name: from, exact: true }).click();
  await page.getByRole("radio", { name: relation.cardinality }).check();
  await page.getByLabel("Target class").selectOption(relation.to);
  await page.getByLabel(`Name on ${from}`).fill(relation.name);
  await page.getByLabel(`Inverse on ${relation.to}`).fill(relation.inverse);
  if (relation.requiredOn) {
    await page.getByRole("checkbox", { name: `Required on ${relation.requiredOn}` }).check();
  }
  if (relation.onDelete) {
    await page.getByRole("radio", { name: relation.onDelete, exact: true }).check();
  }
  await page.getByRole("button", { name: "Add relationship" }).click();
  await expect(page.getByRole("table", { name: "Relationships" })).toContainText(relation.name);
}

/** One write through the Endpoint with the person's own session, as the SDK sends it. */
async function write(who: Session, method: "POST" | "PATCH", path: string, body: unknown): Promise<{ status: number; rule?: string; detail?: string }> {
  const answer = await who.page.request.fetch(`/api/endpoint/${slug}/ngsi-ld/v1${path}`, {
    method,
    headers: { "content-type": "application/json", "x-csrf-token": await csrf(who.context) },
    data: body,
  });
  const text = await answer.text();
  let problem: { rule?: string; detail?: string } = {};
  try {
    problem = JSON.parse(text) as { rule?: string; detail?: string };
  } catch {
    // A 201 or 204 carries no body.
  }
  return { status: answer.status(), rule: problem.rule, detail: problem.detail };
}

const entity = (type: string, local: string, attrs: Record<string, unknown> = {}) => ({
  id: urn(type, local),
  type,
  name: { type: "Property", value: `${type} ${local}` },
  ...attrs,
});
const rel = (object: string | string[]) => ({ type: "Relationship", object });

test.beforeAll(async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}?lang=en`);
  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}?lang=en`);
});

test.afterAll(async () => {
  const page = steward.page;
  await removeCompletely(steward, PROJECT, "policies", CREATE);
  await removeCompletely(steward, PROJECT, "endpoints", ENDPOINT);
  for (const plural of ["projections", "policies"]) {
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
    const items = answer.ok() ? (((await answer.json()) as { items?: { metadata: { name: string } }[] }).items ?? []) : [];
    for (const { metadata } of items.filter((one) => one.metadata.name.startsWith(ENDPOINT))) {
      await removeCompletely(steward, PROJECT, plural, metadata.name);
    }
  }
  await removeCompletely(steward, PROJECT, "spaces", SPACE);
  await removeCompletely(steward, PROJECT, "datamodels", MODEL);
  await sweepDrafts(steward.context, page, PROJECT, MINE);
  for (const session of [steward, approver, viewer]) {
    await session.context.close();
  }
});

test("1. a steward builds all four cardinalities in the model editor and publishes them to a new space", async () => {
  test.setTimeout(900_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: /^New (context )?space/i }).first().click();
  const form = page.getByTestId("form-page");
  await form.getByLabel(/^Name/).fill(SPACE);
  await proposeFrom(form);
  await approveAsked(await proposedChange(page));
  await listed(page, "spaces", SPACE);

  await page.goto(`/projects/${PROJECT}/models?new=blank&space=${SPACE}&lang=en`, { waitUntil: "load" });
  const editor = page.getByRole("region", { name: "New model" });
  await editor.getByLabel("Name", { exact: true }).fill(MODEL);
  await editor.getByLabel("Space", { exact: true }).selectOption(SPACE);
  for (const name of CLASSES) {
    await page.getByLabel("New type").fill(name);
    await page.getByRole("button", { name: "Add type" }).click();
    await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
  }
  await relate(page, "School", { cardinality: "One to many", to: "User", name: "users", inverse: "school", requiredOn: "User" });
  await relate(page, "User", { cardinality: "Many to many", to: "Course", name: "courses", inverse: "students" });
  await relate(page, "User", { cardinality: "One to one", to: "Profile", name: "profile", inverse: "owner", onDelete: "Cascade" });
  await relate(page, "Person", { cardinality: "Many to one", to: "Person", name: "manager", inverse: "reports", onDelete: "Set null" });

  await page.getByRole("button", { name: "Check" }).click();
  await expect(page.getByText(/Saves as version/)).toBeVisible({ timeout: 60_000 });
  const saved = page.waitForRequest(
    (request) => request.method() === "PUT" && /\/datamodels\/[^/]+\/source/.test(request.url()) && !request.url().includes("dryRun"),
  );
  await page.getByRole("button", { name: /^Propose v\d/ }).click();
  const source = (await saved).postData() ?? "";
  // Both ends of each, the delete rule on the source end, required only on the stored end.
  for (const part of ["inverse: school", "inverse: users", "inverse: students", "inverse: owner", "inverse: reports", "on_delete: cascade", "on_delete: set-null", "required: true"]) {
    expect(source, `the saved source carries ${part}`).toContain(part);
  }
  await approveAsked(await proposedChange(page));
  await listed(page, "datamodels", MODEL);
});

test("2. an endpoint over the space reads and updates every class, and the project's writers may create them", async () => {
  test.setTimeout(1_200_000);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/endpoints?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "New endpoint" }).first().click();
  const form = page.getByTestId("form-page");
  await form.locator("#root_name").fill(ENDPOINT);
  await form.locator("#root_title").fill(`Relationships ${SUFFIX}`);
  await form.locator("#root_contextSpaceRef").selectOption(SPACE);
  await form.locator("#root_audience").selectOption({ label: "Public: anyone with the address" });
  for (const name of CLASSES) {
    await form.getByRole("checkbox", { name, exact: true }).check();
    await form.getByRole("checkbox", { name: `${name} Writable` }).check();
  }
  await proposeFrom(form);
  await approveAsked(await proposedChange(page));

  // The endpoint form grants updateOps, which holds no create (CIM 009 Table 4.20-2); creating is
  // its own grant, to the same writers.
  const policy = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: CREATE, namespace: PROJECT },
    spec: {
      contextSpaceRef: { kind: "ContextSpace", name: SPACE },
      assigner: "did:web:{orgDomain}",
      assignee: { kind: "role", id: `${PROJECT}-writers` },
      operations: ["createEntity"],
      information: CLASSES.map((type) => ({ entities: [{ type }] })),
    },
  };
  await checkManifest(page, steward.context, PROJECT, policy);
  const answer = await page.request.post(`/api/v1/projects/${PROJECT}/policies`, {
    headers: { "x-csrf-token": await csrf(steward.context) },
    data: policy,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  expect(change, "the policy's proposal names its change").not.toBe("");
  await approveAsked(change);

  await expect
    .poll(
      async () => {
        const endpoint = await page.request.get(`/api/v1/projects/${PROJECT}/endpoints/${ENDPOINT}`);
        slug = endpoint.ok() ? (((await endpoint.json()) as { spec?: { slug?: string } }).spec?.slug ?? "") : "";
        if (slug === "") return -1;
        return (await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?type=School&limit=1`)).status();
      },
      { timeout: 300_000, intervals: [10_000], message: `${ENDPOINT} answers` },
    )
    .toBe(200);
});

test("3. entities are written through the endpoint, and a school is changed in Explore with the picker", async () => {
  test.setTimeout(600_000);
  const created = [
    entity("School", "north"),
    entity("School", "south"),
    entity("Course", "math"),
    entity("Course", "art"),
    entity("Profile", "ana"),
    entity("Person", "boss"),
    entity("User", "ana", { school: rel(urn("School", "north")), courses: rel([urn("Course", "math"), urn("Course", "art")]), profile: rel(urn("Profile", "ana")) }),
    entity("Person", "eva", { manager: rel(urn("Person", "boss")) }),
  ];
  // Targets before the entities that point at them: the gateway reads each target (target-missing).
  for (const one of created) {
    const answer = await write(steward, "POST", "/entities", one);
    expect(answer.status, `${one.id}: ${answer.detail ?? ""}`).toBe(201);
  }

  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/explore?space=${SPACE}&endpoint=${ENDPOINT}&type=User&lang=en`, { waitUntil: "load" });
  const row = page.getByRole("row").filter({ hasText: "User ana" });
  const school = row.getByRole("group", { name: "Edit school" });
  await expect(school).toBeVisible({ timeout: 120_000 });
  const box = school.getByRole("combobox", { name: "Search School" });
  await box.focus();
  await box.fill("south");
  await school.getByRole("option", { name: /south/ }).click();
  await page.getByRole("button", { name: "Review the changes" }).click();
  const patched = page.waitForRequest((request) => request.method() === "PATCH" && request.url().includes(encodeURIComponent(urn("User", "ana"))));
  await page.getByRole("button", { name: "Apply" }).click();
  const body = JSON.parse((await patched).postData() ?? "{}") as Record<string, unknown>;
  expect(body.school).toEqual({ type: "Relationship", object: urn("School", "south") });

  await expect
    .poll(async () => {
      const answer = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities/${encodeURIComponent(urn("User", "ana"))}?options=keyValues`);
      return answer.ok() ? ((await answer.json()) as { school?: unknown }).school : answer.status();
    }, { timeout: 60_000, message: "the endpoint holds the picked school" })
    .toBe(urn("School", "south"));
});

test("4. every violation the gateway holds is refused with its rule, and a viewer's write is refused", async () => {
  test.setTimeout(300_000);
  const user = (local: string, attrs: Record<string, unknown>) => entity("User", local, attrs);
  const cases: [string, "POST" | "PATCH", string, unknown, string][] = [
    ["a wrong type", "POST", "/entities", user("wrong", { school: rel(urn("Course", "math")) }), "target-wrong-type"],
    ["a second school", "POST", "/entities", user("two", { school: rel([urn("School", "north"), urn("School", "south")]) }), "single-end-many-targets"],
    ["a missing required end", "POST", "/entities", user("none", {}), "required-end-missing"],
    ["a school the space does not hold", "POST", "/entities", user("ghost", { school: rel(urn("School", "nowhere")) }), "target-missing"],
    ["the required end emptied", "PATCH", `/entities/${encodeURIComponent(urn("User", "ana"))}/attrs`, { school: rel("urn:ngsi-ld:null") }, "required-end-missing"],
  ];
  for (const [what, method, path, body, rule] of cases) {
    const answer = await write(steward, method, path, body);
    expect(answer.status, `${what}: ${answer.detail ?? ""}`).toBe(400);
    expect(answer.rule, what).toBe(rule);
    // The refusal says what to do in words, and names the end it is about.
    expect(answer.detail ?? "", what).toContain("`school`");
  }

  // Nothing was written by a refused write.
  for (const local of ["wrong", "two", "none", "ghost"]) {
    const answer = await steward.page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities/${encodeURIComponent(urn("User", local))}`);
    expect(answer.status(), `User ${local} was written`).toBe(404);
  }

  // A viewer reads the public endpoint and may write nothing through it.
  const refused = await write(viewer, "PATCH", `/entities/${encodeURIComponent(urn("User", "ana"))}/attrs`, { school: rel(urn("School", "north")) });
  expect(refused.status, refused.detail ?? "").toBe(403);
});
