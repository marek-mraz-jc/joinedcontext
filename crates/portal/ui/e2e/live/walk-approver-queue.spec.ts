/**
 * T-5539 — a department head works through a queue of ten Changes from colleagues and an MCP
 * client, on dev, timed: find the queue, read each plan, approve the yellow ones, type the name for
 * the red ones, reject one with a reason, meet the collision, undo a merged Change, and measure the
 * time from each approval to the reconciler's `Applied`.
 *
 * Persona map (decided by worker-t5539, owner may override): a throwaway project with its own walk
 * people needs the org-admin flows T-5535 walks, and the seeded MCP account is bound in helsinki
 * only, so this walk runs in helsinki on DataSources named `walk-approver-…-HHMM` (the janitor's
 * name pattern, so the residue sweep removes what a failed run leaves). A DataSource without a
 * Pipeline reads nothing, so no data moves during the walk.
 *   - the department head is demo.steward (TOTP, PORTAL_TOTP): decides the queue;
 *   - colleague one is demo.editor: proposes creates and updates, approves nothing of their own;
 *   - colleague two is demo.janitor: proposes the red deletions (delete without approve-at-propose);
 *   - the head's own access change: a Group demo.steward proposes and may not approve (PF-58);
 *   - the MCP client is ServiceAccount helsinki/pipeline-proposer (PROPOSER_CLIENT_SECRET).
 *
 * The step table goes to the report as `walk-approver.json`; SHOTS_DIR collects 1440 and 400 shots.
 *
 *   PORTAL_PASSWORD=… PORTAL_TOTP=… EDITOR_PASSWORD=… JANITOR_PASSWORD=… PROPOSER_CLIENT_SECRET=… \
 *   SHOTS_DIR=… npx playwright test --config playwright.live.config.ts e2e/live/walk-approver-queue.spec.ts
 */
import { expect, test } from "@playwright/test";
import type { APIResponse, BrowserContext, Page } from "@playwright/test";
import { EDITOR, JANITOR, STEWARD, approve, csrf, reject, removeCompletely, serviceAccountToken, signIn } from "./portal";
import { FEED } from "./loadFeed";
import { shot } from "./shoot";

type Session = { context: BrowserContext; page: Page };
type Row = { step: string; lapS: number; clicks: number; verdict: string };
type Queued = { id: string; source: string; what: string; lane?: string; approvedAt?: number; appliedS?: number };
type ToolResult = { isError?: boolean; status?: string; content?: { text?: string }[]; structuredContent?: Record<string, unknown>; _meta?: Record<string, unknown>; task?: { taskId: string } };

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const name = (part: string): string => `walk-approver-${part}-${SUFFIX}`;
const BASE_UNDO = name("undo");
const BASE_CLASH = name("clash");
const BASE_GONE = name("gone");
const BASE_GONE_MCP = name("gone-mcp");
const GROUP = name("team");
const REASON = "Walk T-5539: this source duplicates the bike feed already in helsinki; reuse that one.";

const rows: Row[] = [];
const queue: Queued[] = [];
const findings: string[] = [];
let started = 0;
let clicks = 0;
let head: Session;
let editor: Session;
let janitor: Session;
const originals: Record<string, string> = {};
let groupChange = "";

test.describe.configure({ mode: "serial" });
test.use({ trace: "off" });

function source(metaName: string, timeout = "15s") {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: metaName, namespace: PROJECT },
    spec: { type: "http", http: { url: FEED, verb: "GET", timeout } },
  };
}

function lap(step: string, verdict: string): void {
  rows.push({ step, lapS: Math.round((Date.now() - started) / 1000), clicks, verdict });
  test.info().annotations.push({ type: "lap", description: `${step}: ${rows.at(-1)?.lapS} s, ${clicks} clicks, ${verdict}` });
  clicks = 0;
}

async function shots(page: Page, route: string, state: string): Promise<void> {
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: size.height });
    await shot(page, { area: "walk-approver", route, state, who: "steward", lang: "en" });
  }
  await page.setViewportSize(size);
}

async function send(who: Session, method: "post" | "put" | "delete", path: string, data?: unknown): Promise<APIResponse> {
  return who.page.request[method](`/api/v1/projects/${path}`, { headers: { "x-csrf-token": await csrf(who.context) }, data });
}

/** The Change a 202 names; anything else fails with the body the person would read. */
async function changeOf(answer: APIResponse, what: string): Promise<{ id: string; phase?: string }> {
  expect(answer.status(), `${what}: ${await answer.text()}`).toBe(202);
  const body = (await answer.json()) as { metadata?: { name?: string }; status?: { phase?: string } };
  return { id: body.metadata?.name ?? "", phase: body.status?.phase };
}

async function changeState(id: string, project = PROJECT): Promise<{ phase?: string; lane?: string; author?: string; decision?: { by?: string; reason?: string } }> {
  const answer = await head.page.request.get(`/api/v1/projects/${project}/changes/${id}`);
  const body = (await answer.json()) as { status?: { phase?: string; lane?: string }; author?: { email?: string }; decision?: { by?: string; reason?: string } };
  return { phase: body.status?.phase, lane: body.status?.lane, author: body.author?.email, decision: body.decision };
}

/** Waits until the reconciler applied `id` (or the forge merged it, when nothing reports Applied). */
async function applied(id: string): Promise<string | undefined> {
  let phase: string | undefined;
  await expect
    .poll(async () => (phase = (await changeState(id)).phase), { timeout: 600_000, intervals: [3_000], message: `${id} reaches Applied` })
    .toBe("Applied")
    .catch(() => findings.push(`${id} stayed ${phase} for 10 min after its approval`));
  return phase;
}

/** The persisted manifest of a merged Change, the spec and metadata Git holds, as one string. */
async function written(id: string): Promise<string> {
  const answer = await head.page.request.get(`/api/v1/projects/${PROJECT}/changes/${id}/manifests`);
  expect(answer.status(), `manifests of ${id}: ${await answer.text()}`).toBe(200);
  const items = ((await answer.json()) as { items?: { spec?: unknown; metadata?: { name?: string } }[] }).items ?? [];
  return JSON.stringify(items.map((item) => ({ name: item.metadata?.name, spec: item.spec })));
}

function mcpClient(baseURL: string, token: string) {
  let id = 0;
  const rpc = async (method: string, params: Record<string, unknown>): Promise<{ result?: ToolResult }> => {
    id += 1;
    const answer = await fetch(new URL("/api/v1/mcp", baseURL), {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
    });
    const text = await answer.text();
    expect(answer.status, `${method}: ${text.slice(0, 500)}`).toBe(200);
    return JSON.parse(text) as { result?: ToolResult };
  };
  const call = async (tool: string, args: Record<string, unknown>): Promise<ToolResult> => {
    const params = { name: tool, project: PROJECT, arguments: { project: PROJECT, ...args } };
    let result = (await rpc("tools/call", params)).result ?? {};
    const asked = (result._meta?.["joinedcontext.com/elicitation"] as { elicitation?: { elicitationId?: string } } | undefined)?.elicitation;
    if (result.status === "input_required" && asked?.elicitationId) {
      result = (await rpc("tools/call", { ...params, elicitation: { elicitationId: asked.elicitationId, action: "accept" } })).result ?? {};
    }
    if (result.task) {
      const taskId = result.task.taskId;
      await expect.poll(async () => (await rpc("tasks/get", { taskId })).result?.status, { timeout: 180_000, intervals: [2_000] }).not.toBe("working");
      result = (await rpc("tasks/result", { taskId })).result ?? {};
    }
    return result;
  };
  return call;
}

const said = (result: ToolResult): string => (result.content ?? []).map((part) => part.text ?? "").join(" ");

/** Rejects every walk Change still open and removes every walk resource, as the head. */
async function cleanUp(owner: Session): Promise<void> {
  if (groupChange) await send(owner, "post", `org/changes/${groupChange}/reject`, { reason: "Walk T-5539 cleanup" });
  for (const project of [PROJECT, "org"]) {
    const answer = await owner.page.request.get(`/api/v1/projects/${project}/changes`);
    if (!answer.ok()) continue;
    const items = ((await answer.json()) as { items?: { metadata: { name: string } }[] }).items ?? [];
    for (const item of items.filter((one) => JSON.stringify(one).includes("walk-approver-"))) {
      await send(owner, "post", `${project}/changes/${item.metadata.name}/reject`, { reason: "Walk T-5539 cleanup" });
    }
  }
  for (const each of [BASE_UNDO, BASE_CLASH, BASE_GONE, BASE_GONE_MCP, ...["a", "b", "c", "d", "mcp"].map(name)]) {
    await removeCompletely(owner, PROJECT, "datasources", each).catch((err: unknown) => findings.push(`cleanup ${each}: ${String(err)}`));
  }
}

test.afterAll(async ({ browser }) => {
  const report = { rows, queue, findings };
  test.info().attachments.push({ name: "walk-approver.json", contentType: "application/json", body: Buffer.from(JSON.stringify(report, null, 2)) });
  console.log(JSON.stringify(report));
  const owner = head ?? (await signIn(browser, STEWARD, `/projects/${PROJECT}/home?lang=en`));
  try {
    await cleanUp(owner);
    const open = await owner.page.request.get(`/api/v1/projects/${PROJECT}/changes`);
    expect(JSON.stringify(await open.json()), "a walk Change is left open").not.toContain("walk-approver-");
  } finally {
    for (const one of [editor, janitor, owner]) await one?.context.close();
  }
});

test("a department head decides a queue of ten changes from colleagues and an MCP client", async ({ browser, baseURL }) => {
  test.setTimeout(90 * 60_000);
  const base = baseURL ?? "https://portal.dev.joinedcontext.com";
  head = await signIn(browser, STEWARD, `/projects/${PROJECT}/home?lang=en`);
  editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/home?lang=en`);
  janitor = await signIn(browser, JANITOR, `/projects/${PROJECT}/home?lang=en`);
  const token = await serviceAccountToken(base);
  const mcp = mcpClient(base, token);

  // Setup: four sources the queue updates, deletes and collides on, proposed by the head, who
  // administers DataSource and so has them approved as proposed (PF-58).
  for (const base of [BASE_UNDO, BASE_CLASH, BASE_GONE, BASE_GONE_MCP]) {
    const made = await changeOf(await send(head, "post", `${PROJECT}/datasources`, source(base)), `create ${base}`);
    if (made.phase === "PendingApproval") await approve(janitor.page, PROJECT, made.id);
    originals[base] = made.id;
  }
  for (const id of Object.values(originals)) await applied(id);

  // The queue: ten proposals from three people and the MCP account.
  const propose = async (sourceName: string, what: string, answer: Promise<APIResponse>) => {
    queue.push({ id: (await changeOf(await answer, what)).id, source: sourceName, what });
  };
  for (const part of ["a", "b", "c", "d"]) await propose("demo.editor", `create ${name(part)}`, send(editor, "post", `${PROJECT}/datasources`, source(name(part))));
  await propose("demo.editor", `update ${BASE_UNDO}`, send(editor, "put", `${PROJECT}/datasources/${BASE_UNDO}`, source(BASE_UNDO, "20s")));
  await propose("demo.editor", `update ${BASE_CLASH}`, send(editor, "put", `${PROJECT}/datasources/${BASE_CLASH}`, source(BASE_CLASH, "25s")));
  await propose("demo.janitor", `delete ${BASE_GONE}`, send(janitor, "delete", `${PROJECT}/datasources/${BASE_GONE}`));
  await propose("demo.steward", `create Group ${GROUP}`, send(head, "post", "org/groups", {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Group",
    metadata: { name: GROUP, namespace: "org" },
    spec: { description: "The T-5539 walk's team; bound by nothing, rejected at the end", members: [{ user: EDITOR.user }] },
  }));
  const group = queue.at(-1);
  if (group) group.what = `org:${group.what}`;
  groupChange = group?.id ?? "";
  const viaMcp = await mcp("jc_resource_propose", { manifest: source(name("mcp")) });
  expect(viaMcp.isError, `MCP proposal refused: ${said(viaMcp)}`).not.toBe(true);
  queue.push({ id: String(viaMcp.structuredContent?.changeId ?? ""), source: "mcp:pipeline-proposer", what: `create ${name("mcp")}` });
  const mcpDelete = await mcp("jc_resource_delete", { kind: "DataSource", name: BASE_GONE_MCP, confirm: BASE_GONE_MCP });
  const mcpDeleteChange = mcpDelete.structuredContent?.changeId;
  if (typeof mcpDeleteChange === "string" && mcpDeleteChange.startsWith("chg-")) {
    queue.push({ id: mcpDeleteChange, source: "mcp:pipeline-proposer", what: `delete ${BASE_GONE_MCP}` });
  } else {
    // pipeline-editor holds propose and no delete: a red deletion from this account is refused.
    findings.push(`MCP jc_resource_delete refused: ${said(mcpDelete).slice(0, 300)}`);
    await propose("demo.janitor", `delete ${BASE_GONE_MCP}`, send(janitor, "delete", `${PROJECT}/datasources/${BASE_GONE_MCP}`));
  }
  // (6) The colliding proposal: the MCP account changes the source the editor's open Change holds.
  const clash = await mcp("jc_resource_propose", { manifest: source(BASE_CLASH, "30s") });
  const clashChange = queue.find((one) => one.what === `update ${BASE_CLASH}`)?.id ?? "";
  expect(clash.isError, "a second proposal on a resource with an open Change is refused (CC-34)").toBe(true);
  expect(said(clash), "the refusal names the Change to decide first").toContain(clashChange);
  const clashRest = await send(editor, "put", `${PROJECT}/datasources/${BASE_CLASH}`, source(BASE_CLASH, "35s"));
  expect(clashRest.status(), "the same refusal on REST").toBe(409);
  expect(await clashRest.text()).toContain(clashChange);
  expect(queue.length, "ten Changes wait").toBeGreaterThanOrEqual(10);
  for (const one of queue) one.lane = (await changeState(one.id, one.what.startsWith("org:") ? "org" : PROJECT)).lane;
  started = Date.now();

  // 1. Find what waits: from the project home to the queue.
  await head.page.goto(`/projects/${PROJECT}/home?lang=en`, { waitUntil: "load" });
  const badge = await head.page.getByText(/changes? waits? for your approval/).first().textContent({ timeout: 15_000 }).catch(() => null);
  const nav = head.page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: /^Approvals/ });
  if (!(await nav.isVisible())) {
    clicks++;
    await head.page.getByRole("button", { name: /menu|navigation/i }).first().click();
  }
  clicks++;
  await nav.click();
  await expect(head.page).toHaveURL(new RegExp(`/projects/${PROJECT}/approvals`));
  const listed = await head.page.locator("main").innerText();
  const missing = queue.filter((one) => !one.what.startsWith("org:") && !listed.includes(one.id));
  expect.soft(missing.map((one) => one.id), "every helsinki walk Change is in the head's queue").toEqual([]);
  await shots(head.page, `/projects/${PROJECT}/approvals`, "queue");
  lap("1. find the queue", `badge: ${badge ?? "none"}; ${queue.length} waiting`);

  // 2. Read each plan diff and lane; the head's own access change refuses their approval.
  for (const one of queue) {
    const project = one.what.startsWith("org:") ? "org" : PROJECT;
    clicks++;
    await head.page.goto(`/projects/${project}/approvals/${one.id}?lang=en`, { waitUntil: "load" });
    await expect(head.page.getByRole("button", { name: "Reject", exact: true })).toBeVisible({ timeout: 60_000 });
  }
  const update = queue.find((one) => one.what === `update ${BASE_UNDO}`);
  await head.page.goto(`/projects/${PROJECT}/approvals/${update?.id}?lang=en`, { waitUntil: "load" });
  await expect(head.page.getByText(/20s/).first(), "the plan diff shows the new timeout").toBeVisible({ timeout: 60_000 });
  await shots(head.page, `/projects/${PROJECT}/approvals/${update?.id}`, "plan-diff");
  const own = await send(head, "post", `org/changes/${group?.id}/approve`, { confirm: GROUP });
  expect(own.status(), "the head approves their own access change").toBe(403);
  expect(await own.text()).toContain("self-approval");
  const editorsOwn = await send(editor, "post", `${PROJECT}/changes/${queue[0].id}/approve`, {});
  expect(editorsOwn.status(), "the editor approves their own change").toBe(403);
  const bearer = await fetch(new URL(`/api/v1/projects/${PROJECT}/changes/${queue[0].id}/approve`, base), {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: "{}",
  });
  expect(bearer.status, "a token approves").toBe(403);
  const mcpApprove = await mcp("jc_change_approve", { change: queue[0].id });
  expect(mcpApprove.isError, "the MCP account approves").toBe(true);
  lap("2. read plans and lanes", queue.map((one) => `${one.id}:${one.lane}`).join(" "));

  // 3. Approve the yellow ones (all but the rejected one), each timed to Applied.
  const rejected = queue.find((one) => one.what === `create ${name("b")}`);
  const yellow = queue.filter((one) => one.lane !== "red" && one !== rejected && !one.what.startsWith("org:"));
  for (const one of yellow) {
    clicks += 2;
    await approve(head.page, PROJECT, one.id);
    one.approvedAt = Date.now();
  }
  for (const one of yellow) {
    await applied(one.id);
    one.appliedS = Math.round((Date.now() - (one.approvedAt ?? Date.now())) / 1000);
  }
  lap("3. approve the yellow ones", `${yellow.length} approved`);

  // 4. Red: Approve stays off until the name is typed; the API refuses an untyped approval.
  const reds = queue.filter((one) => one.lane === "red" && !one.what.startsWith("org:"));
  for (const one of reds) {
    const target = one.what.split(" ").at(-1) ?? "";
    const untyped = await send(head, "post", `${PROJECT}/changes/${one.id}/approve`, {});
    expect(untyped.status(), `${one.id} approved without the typed name`).toBeGreaterThanOrEqual(400);
    await head.page.goto(`/projects/${PROJECT}/approvals/${one.id}?lang=en`, { waitUntil: "load" });
    await expect(head.page.locator("#confirm-resource-name")).toBeVisible({ timeout: 60_000 });
    await expect(head.page.getByRole("button", { name: "Approve", exact: true })).toBeDisabled();
    if (one === reds[0]) await shots(head.page, `/projects/${PROJECT}/approvals/${one.id}`, "red-confirmation");
    clicks += 3;
    await approve(head.page, PROJECT, one.id, target);
    one.approvedAt = Date.now();
  }
  for (const one of reds) {
    await applied(one.id);
    one.appliedS = Math.round((Date.now() - (one.approvedAt ?? Date.now())) / 1000);
  }
  lap("4. approve the red ones", `${reds.length} approved with the name typed`);

  // 5. Reject one with a reason; its author reads the reason.
  clicks += 3;
  await reject(head.page, PROJECT, rejected?.id ?? "", REASON);
  await editor.page.goto(`/projects/${PROJECT}/approvals/${rejected?.id}?lang=en`, { waitUntil: "load" });
  await expect(editor.page.getByText(REASON).first(), "the author sees the reason").toBeVisible({ timeout: 60_000 });
  await shots(editor.page, `/projects/${PROJECT}/approvals/${rejected?.id}`, "rejected-as-author");
  const asAuthor = await editor.page.request.get(`/api/v1/projects/${PROJECT}/changes/${rejected?.id}`);
  expect(((await asAuthor.json()) as { decision?: { reason?: string } }).decision?.reason).toBe(REASON);
  lap("5. reject with a reason", "the author reads it on the change page and in the API");

  // 6. After the editor's Change merged, the MCP account may propose on the same source again.
  const afterClash = await mcp("jc_resource_propose", { manifest: source(BASE_CLASH, "30s") });
  expect(afterClash.isError, `the second author retries after the merge: ${said(afterClash)}`).not.toBe(true);
  const retry = String(afterClash.structuredContent?.changeId ?? "");
  queue.push({ id: retry, source: "mcp:pipeline-proposer", what: `update ${BASE_CLASH} (retry)` });
  await approve(head.page, PROJECT, retry);
  lap("6. collision", `refused while ${clashChange} was open, naming it; the retry ${retry} approved`);

  // 7. Undo the merged update: the author proposes it, the head approves, and the manifest Git
  // holds is the one before the update.
  const undo = await changeOf(await send(editor, "post", `${PROJECT}/changes/${update?.id}/undo`), `undo ${update?.id}`);
  expect(undo.phase, "the undo is itself a Change").toBe("PendingApproval");
  clicks += 2;
  await approve(head.page, PROJECT, undo.id);
  await applied(undo.id);
  expect(await written(undo.id), "undo restores the manifest").toBe(await written(originals[BASE_UNDO]));
  queue.push({ id: undo.id, source: "demo.editor", what: `undo ${update?.id}` });
  lap("7. undo a merged change", `${undo.id} restored ${BASE_UNDO}`);

  // 8. Every decision on the activity feed, naming the approver.
  const feed = await head.page.request.get(`/api/v1/projects/${PROJECT}/activity?kind=change.merged&limit=200`);
  const events = ((await feed.json()) as { items?: { correlationId?: string; details?: { approvedBy?: string } }[] }).items ?? [];
  for (const one of [...yellow, ...reds]) {
    expect.soft(events.find((event) => event.correlationId === one.id)?.details?.approvedBy, `activity names the approver of ${one.id}`).toBe(STEWARD.user);
  }
  const all = await head.page.request.get(`/api/v1/projects/${PROJECT}/activity?limit=500`);
  if (!JSON.stringify(await all.json()).includes(rejected?.id ?? "-")) findings.push(`the rejection of ${rejected?.id} is not on the activity feed`);
  expect.soft(findings.filter((one) => one.startsWith("the rejection")), "the rejection is on the activity feed").toEqual([]);
  const times = [...yellow, ...reds].map((one) => one.appliedS ?? 0).sort((a, b) => a - b);
  lap("8. applied and recorded", `approve to Applied p50 ${times[Math.floor(times.length / 2)]} s, max ${times.at(-1)} s`);
});
