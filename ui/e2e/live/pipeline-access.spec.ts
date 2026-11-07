/**
 * A pipeline's access to another space on dev (T-4863, PL-90…PL-93, API/01 §47): the steward asks
 * for one of Helsinki's pipelines to read an Endpoint of another Helsinki space, naming the type
 * and attributes; the request grants nothing, the steward cannot approve it, the approver approves
 * it as a Change the steward then approves (four eyes), and the approver revokes it again.
 *
 * It reads the first pipeline and the first Endpoint of a different space the project holds, so it
 * writes nothing but the request and the two Changes; the revoke leaves the grant ended.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, csrf, signIn } from "./portal";

type Session = { context: BrowserContext; page: Page };
type Item = { metadata: { name: string }; spec: { contextSpaceRef?: string | { name?: string } } };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");

let steward: Session;
let approver: Session;

test.describe.configure({ mode: "serial" });
test.setTimeout(600_000);

const spaceOf = (item: Item) =>
  typeof item.spec.contextSpaceRef === "string" ? item.spec.contextSpaceRef : (item.spec.contextSpaceRef?.name ?? "");

async function post(session: Session, path: string, data?: unknown) {
  return session.page.request.post(path, { headers: { "X-CSRF-Token": await csrf(session.context) }, data: data ?? {} });
}

async function changeOf(answer: { status(): number; text(): Promise<string>; json(): Promise<unknown> }): Promise<string> {
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  expect(change, "the answer names its change").not.toBe("");
  return change;
}

test.beforeAll(async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/pipelines?lang=en`);
});

test.afterAll(async () => {
  await steward?.context.close();
  await approver?.context.close();
});

test("a steward asks another space's owner for a pipeline's read, the owner approves it and revokes it", async () => {
  const list = async (plural: string) =>
    ((await (await steward.page.request.get(`/api/v1/projects/${PROJECT}/${plural}`)).json()) as { items: Item[] }).items;
  const [pipeline] = await list("pipelines");
  expect(pipeline, "Helsinki holds a pipeline").toBeDefined();
  const endpoint = (await list("endpoints")).find((item) => spaceOf(item) !== spaceOf(pipeline));
  expect(endpoint, "Helsinki holds an Endpoint of another space than the pipeline's").toBeDefined();
  const requests = `/api/v1/projects/${PROJECT}/pipelines/${pipeline.metadata.name}/access-requests`;

  const filed = await post(steward, requests, {
    endpoint: { project: PROJECT, name: endpoint!.metadata.name },
    operations: "read",
    types: [{ type: "AirQualityObserved", attrs: ["pm10"] }],
    reason: `T-4863 live journey ${SUFFIX}`,
  });
  expect(filed.status(), await filed.text()).toBe(201);
  const { id, state } = (await filed.json()) as { id: string; state: string };
  expect(state).toBe("pending");

  // The requester never approves their own request (PL-91).
  const own = await post(steward, `/api/v1/projects/${PROJECT}/access-requests/${id}/approve`);
  expect(own.status(), await own.text()).toBe(403);

  // The owner approves: one Change of Helsinki, which another person approves (T-4833).
  const granted = await changeOf(await post(approver, `/api/v1/projects/${PROJECT}/access-requests/${id}/approve`));
  await approve(steward.page, PROJECT, granted);
  const mine = (await (await steward.page.request.get(requests)).json()) as { items: { id: string; state: string }[] };
  expect(mine.items.find((item) => item.id === id)?.state).toBe("approved");

  // Revoking needs a reason and ends the grant in one more Change (PL-93).
  const bare = await post(approver, `/api/v1/projects/${PROJECT}/access-requests/${id}/revoke`, { reason: " " });
  expect(bare.status()).toBe(400);
  await expect
    .poll(async () => (await post(approver, `/api/v1/projects/${PROJECT}/access-requests/${id}/revoke`, { reason: `T-4863 ${SUFFIX} done` })).status(), {
      timeout: 300_000,
      intervals: [10_000],
      message: "the grant reaches main and the revoke proposes its end",
    })
    .toBe(202);
  const after = (await (await steward.page.request.get(requests)).json()) as { items: { id: string; state: string }[] };
  expect(after.items.find((item) => item.id === id)?.state).toBe("revoked");
});

test("a declined request carries the owner's reason back to the requester", async () => {
  const [pipeline] = ((await (await steward.page.request.get(`/api/v1/projects/${PROJECT}/pipelines`)).json()) as { items: Item[] }).items;
  const endpoints = ((await (await steward.page.request.get(`/api/v1/projects/${PROJECT}/endpoints`)).json()) as { items: Item[] }).items;
  const endpoint = endpoints.find((item) => spaceOf(item) !== spaceOf(pipeline));
  const requests = `/api/v1/projects/${PROJECT}/pipelines/${pipeline.metadata.name}/access-requests`;
  const filed = await post(steward, requests, {
    endpoint: { project: PROJECT, name: endpoint!.metadata.name },
    operations: "write",
    types: [{ type: "AirQualityObserved", attrs: ["pm10"] }],
    reason: `T-4863 decline ${SUFFIX}`,
  });
  expect(filed.status(), await filed.text()).toBe(201);
  const { id } = (await filed.json()) as { id: string };
  const declined = await post(approver, `/api/v1/projects/${PROJECT}/access-requests/${id}/decline`, { reason: "use the read grant" });
  expect(declined.status(), await declined.text()).toBe(200);
  const mine = (await (await steward.page.request.get(requests)).json()) as { items: { id: string; decisionReason: string }[] };
  expect(mine.items.find((item) => item.id === id)?.decisionReason).toBe("use the read grant");
});
