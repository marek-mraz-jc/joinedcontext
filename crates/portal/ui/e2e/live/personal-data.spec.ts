/**
 * A data subject's rights on dev (T-4668, DM-124…DM-126, API/01 §46): the owner of a sandbox space
 * whose model names its data subject exports what the space holds about a person and erases it,
 * each with a reason, and nobody else may. The subject travels in the body, never in a URL.
 *
 *  1. A sandbox space, a model in it whose `Rider` class names `rider` as its `dataSubjectRef`,
 *     and the space pointed at the model, each approved by the steward who proposed it.
 *  2. The export answers the space's Entities naming the subject, `no-store`; a call without a
 *     reason is refused with `reason-required`, and a viewer is refused.
 *  3. A dry run counts what an erasure would take and erases nothing; the erasure answers `202`
 *     `proposed` with the address of its state, which answers per store, and is declined again.
 *
 * The space holds no Entity of the subject: what an erasure takes out of the broker and the
 * conversations is T-4668's own evidence on dev. Names are `t4668-…-HHMM`;
 * `afterAll` removes the space and the model again.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, VIEWER, approve, checkManifest, csrf, removeCompletely, signIn } from "./portal";

type Session = { context: BrowserContext; page: Page };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const SPACE = `t4668-space-${SUFFIX}`;
const MODEL = `t4668-model-${SUFFIX}`;
const SUBJECTS = `/api/v1/projects/${PROJECT}/spaces/${SPACE}`;
const SUBJECT = { subject: `rider-${SUFFIX}`, reason: `T-4668 live journey, request ${SUFFIX}` };
const LINKML = `id: https://example.org/${MODEL}
name: ${MODEL}
prefixes:
  linkml: https://w3id.org/linkml/
default_range: string
imports: [linkml:types, ngsi-ld-core]
slots:
  rider:
    range: string
  riderName:
    range: string
    annotations:
      personalData: true
classes:
  Rider:
    is_a: Entity
    annotations:
      dataSubjectRef: rider
    slots: [rider, riderName]
`;

let steward: Session;
let viewer: Session;

test.describe.configure({ mode: "serial" });
test.setTimeout(600_000);

/** Waits until the project's API answers `path` with 200. */
async function served(page: Page, path: string, what: string): Promise<void> {
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/${path}`)).status(), {
      timeout: 300_000,
      intervals: [5_000],
      message: `${what} reaches the mirror`,
    })
    .toBe(200);
}

/** The change a `202` names, approved by the steward who proposed it (a sandbox's own). */
async function approved(answer: { status(): number; text(): Promise<string>; json(): Promise<unknown> }): Promise<void> {
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  expect(change, "the proposal names its change").not.toBe("");
  await approve(steward.page, PROJECT, change);
}

test.beforeAll(async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  // A run that died before its afterAll left its space and model in the demo project, where
  // every steward saw them in the model list (T-4829): each run first removes what earlier runs
  // left, spaces before the models they name.
  for (const plural of ["spaces", "datamodels"]) {
    const listed = await steward.page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
    const items = listed.ok() ? (((await listed.json()) as { items?: { metadata?: { name?: string } }[] }).items ?? []) : [];
    for (const name of items.map((item) => item.metadata?.name ?? "").filter((name) => /^t4668-(space|model)-\d{4}$/.test(name))) {
      await removeCompletely(steward, PROJECT, plural, name).catch(() => undefined);
    }
  }
});

test.afterAll(async () => {
  if (steward) {
    await removeCompletely(steward, PROJECT, "spaces", SPACE).catch(() => undefined);
    await removeCompletely(steward, PROJECT, "datamodels", MODEL).catch(() => undefined);
  }
  for (const session of [steward, viewer]) await session?.context.close();
});

test("the owner of a space exports and erases a data subject with a reason, and nobody else may", async () => {
  const { page, context } = steward;
  const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };

  // 1. The space, its model naming the subject, and the space pointed at the model.
  const space = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: SPACE, namespace: PROJECT },
    spec: { isSandbox: true },
  };
  await checkManifest(page, context, PROJECT, space);
  await approved(await page.request.post(`/api/v1/projects/${PROJECT}/spaces`, { headers, data: space }));
  await served(page, `spaces/${SPACE}`, `space ${SPACE}`);
  const source = `/api/v1/projects/${PROJECT}/datamodels/${MODEL}/source?space=${SPACE}`;
  const checked = await page.request.put(`${source}&dryRun=All`, { headers: { ...headers, "content-type": "text/yaml" }, data: LINKML });
  expect(checked.ok(), `the model's dry run: ${await checked.text()}`).toBe(true);
  await approved(
    await page.request.put(source, {
      headers: { ...headers, "content-type": "text/yaml" },
      data: LINKML,
    }),
  );
  await served(page, `datamodels/${MODEL}`, `model ${MODEL}`);
  const pointed = { ...space, spec: { ...space.spec, dataModelRef: { kind: "DataModel", name: MODEL } } };
  await checkManifest(page, context, PROJECT, pointed);
  await approved(await page.request.put(`/api/v1/projects/${PROJECT}/spaces/${SPACE}`, { headers, data: pointed }));
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/spaces/${SPACE}`);
        return answer.ok() ? ((await answer.json()) as { spec?: { dataModelRef?: { name?: string } } }).spec?.dataModelRef?.name : undefined;
      },
      { timeout: 300_000, intervals: [5_000], message: "the space names its model" },
    )
    .toBe(MODEL);

  // 2. The export, to the owner's session alone; refused without a reason and to a viewer.
  const exported = await page.request.post(`${SUBJECTS}/subjects/export`, { headers, data: SUBJECT });
  expect(exported.status(), await exported.text()).toBe(200);
  expect(exported.headers()["cache-control"]).toContain("no-store");
  expect(JSON.stringify(await exported.json()), "the subject's value is not echoed back").not.toContain(SUBJECT.subject);

  const unreasoned = await page.request.post(`${SUBJECTS}/subjects/export`, { headers, data: { subject: SUBJECT.subject, reason: " " } });
  expect(unreasoned.status()).toBe(400);
  expect(await unreasoned.text()).toContain("reason-required");

  const viewerHeaders = { "x-csrf-token": await csrf(viewer.context), "content-type": "application/json" };
  for (const route of ["subjects/export", "erasures"]) {
    const refused = await viewer.page.request.post(`${SUBJECTS}/${route}`, { headers: viewerHeaders, data: SUBJECT });
    expect([403, 404], `a viewer's ${route}: ${await refused.text()}`).toContain(refused.status());
  }

  // 3. A dry run erases nothing; the erasure answers with the address of its state.
  const dry = await page.request.post(`${SUBJECTS}/erasures?dryRun=All`, { headers, data: SUBJECT });
  expect(dry.status(), await dry.text()).toBe(200);
  const preview = (await dry.json()) as { dryRun?: boolean; entities?: number };
  expect(preview.dryRun).toBe(true);
  expect(preview.entities).toBe(0);

  const erased = await page.request.post(`${SUBJECTS}/erasures`, { headers, data: SUBJECT });
  expect(erased.status(), await erased.text()).toBe(202);
  const location = erased.headers()["location"] ?? "";
  expect(location).toMatch(new RegExp(`/spaces/${SPACE}/erasures/er-\\d{8}-[0-9a-f]{12}$`));
  const state = await page.request.get(location);
  expect(state.status(), await state.text()).toBe(200);
  const erasure = (await state.json()) as { id?: string; entities?: number; stores?: Record<string, { state?: string }> };
  expect(location.endsWith(erasure.id ?? "-")).toBe(true);
  expect(erasure.entities).toBe(0);
  expect(Object.keys(erasure.stores ?? {})).toContain("broker");
  expect(JSON.stringify(erasure), "no subject value in the erasure").not.toContain(SUBJECT.subject);
  // Four eyes (DM-142): the erasure is a proposal until another owner approves it; the proposer
  // withdraws it here, so the run leaves nothing waiting.
  expect((erasure as { state?: string }).state).toBe("proposed");
  const own = await page.request.post(`${location}/approve`, { headers, data: { subject: SUBJECT.subject } });
  expect(own.status(), await own.text()).toBe(403);
  expect(await own.text()).toContain("self-approval");
  const withdrawn = await page.request.post(`${location}/decline`, { headers, data: { reason: "live journey cleanup" } });
  expect(withdrawn.status(), await withdrawn.text()).toBe(200);
  expect(((await withdrawn.json()) as { state?: string }).state).toBe("declined");
});
