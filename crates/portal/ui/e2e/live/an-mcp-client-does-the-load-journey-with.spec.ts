/**
 * Load from an external MCP client that holds a ServiceAccount token, on dev (T-1595, AG-64, AG-82,
 * AG-11, AG-63, PF-57, PF-58): the seeded account `helsinki/pipeline-proposer` mints a `portal-api`
 * token (T-2245), lists the tools, checks and proposes a DataSource and a Pipeline, is refused the
 * approval and bringing a workspace back, and a person approves each change in the Portal.
 *
 * A Yellow call answers a question first (AG-63): the host shows it to the person and repeats the
 * call with the answer. The person of this journey accepts; what the Portal ran is the answer.
 * Everything the journey made is removed at the end, and a change it left open is rejected, so dev
 * keeps no pending change of it (T-2236).
 */
import { expect, test } from "@playwright/test";
import { FEED, MAPPING } from "./loadFeed";
import { APPROVER, STEWARD, approve, reject, removeCompletely, serviceAccountToken, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
// Journey-named (`t1595-…`), so the residue sweep's janitor may remove what a failed run leaves.
const SOURCE = `t1595-mcp-source-${SUFFIX}`;
const PIPELINE = `t1595-mcp-bikes-${SUFFIX}`;
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";

const SOURCE_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name: SOURCE, namespace: PROJECT },
  spec: { type: "http", http: { url: FEED, verb: "GET", timeout: "15s" } },
};

const PIPELINE_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: PIPELINE, namespace: PROJECT },
  spec: {
    class: "auto",
    period: "60s",
    source: { dataSourceRef: { kind: "DataSource", name: SOURCE } },
    compute: { kind: "bloblang", bloblang: MAPPING },
    targetEndpoint: TARGET,
    output: { type: "Vehicle", mode: "upsert" },
  },
};

interface ToolResult {
  isError?: boolean;
  status?: string;
  content?: { type: string; text?: string }[];
  structuredContent?: Record<string, unknown>;
  _meta?: Record<string, unknown>;
  task?: { taskId: string; status: string };
  tools?: { name: string }[];
}

interface RpcAnswer {
  result?: ToolResult;
  error?: { code: number; message: string };
}

const said = (result: ToolResult): string => (result.content ?? []).map((part) => part.text ?? "").join(" ");

/** The MCP door as a host drives it: JSON-RPC over one POST, the bearer and nothing else. */
function mcpClient(baseURL: string, token: string) {
  let id = 0;
  const rpc = async (method: string, params: Record<string, unknown>): Promise<RpcAnswer> => {
    id += 1;
    const answer = await fetch(new URL("/api/v1/mcp", baseURL), {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
    });
    const text = await answer.text();
    expect(answer.status, `${method}: ${text.slice(0, 500)}`).toBe(200);
    return JSON.parse(text) as RpcAnswer;
  };

  /** One `tools/call`: the question a Yellow call asks answered `accept`, a task polled to its end. */
  const call = async (name: string, args: Record<string, unknown>): Promise<ToolResult> => {
    const params = { name, project: PROJECT, arguments: { project: PROJECT, ...args } };
    let result = (await rpc("tools/call", params)).result ?? {};
    const asked = (result._meta?.["joinedcontext.com/elicitation"] as { elicitation?: { elicitationId?: string } } | undefined)?.elicitation;
    if (result.status === "input_required" && asked?.elicitationId) {
      result = (await rpc("tools/call", { ...params, elicitation: { elicitationId: asked.elicitationId, action: "accept" } })).result ?? {};
    }
    if (result.task) {
      const taskId = result.task.taskId;
      await expect
        .poll(async () => (await rpc("tasks/get", { taskId })).result?.status, { timeout: 180_000, intervals: [2_000] })
        .not.toBe("working");
      result = (await rpc("tasks/result", { taskId })).result ?? {};
    }
    return result;
  };
  return { rpc, call };
}

/** The Change a proposal answered, by its id. */
function changeOf(result: ToolResult): string {
  expect(result.isError, `the proposal was refused: ${said(result)}`).not.toBe(true);
  const id = result.structuredContent?.changeId;
  if (typeof id !== "string" || !id.startsWith("chg-")) {
    throw new Error(`the proposal named no change: ${JSON.stringify(result.structuredContent).slice(0, 500)}`);
  }
  return id;
}

/** Whether the project lists `name` of `plural` yet, read with the account's own token. */
async function listed(baseURL: string, token: string, plural: string, name: string): Promise<boolean> {
  const answer = await fetch(new URL(`/api/v1/projects/${PROJECT}/${plural}`, baseURL), {
    headers: { authorization: `Bearer ${token}` },
  });
  if (!answer.ok) {
    return false;
  }
  const body = (await answer.json()) as { items?: { metadata: { name: string } }[] };
  return (body.items ?? []).some((item) => item.metadata.name === name);
}

test("an MCP client with a ServiceAccount token checks and proposes a data source and a pipeline, is refused approval and bringing a workspace back, and a person approves", async ({ browser, baseURL }) => {
  test.setTimeout(900_000);
  const base = baseURL ?? "";
  const token = await serviceAccountToken(base);
  const mcp = mcpClient(base, token);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}?lang=en`);
  const open = new Set<string>();

  try {
    const hello = await mcp.rpc("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: { elicitation: {} },
      clientInfo: { name: "t1595-live-journey", version: "1" },
    });
    expect(hello.error, JSON.stringify(hello.error)).toBeUndefined();

    // AG-64: what the account may do is what it is offered, and nothing a person decides.
    const tools = ((await mcp.rpc("tools/list", { project: PROJECT })).result?.tools ?? []).map((tool) => tool.name);
    for (const offered of ["jc_datasource_check", "jc_pipeline_test", "jc_resource_propose"]) {
      expect(tools, offered).toContain(offered);
    }
    for (const withheld of ["jc_change_approve", "jc_workspace_propose"]) {
      expect(tools, withheld).not.toContain(withheld);
    }

    // The data source: checked (PF-57), proposed, and not approved by the account.
    const sourceCheck = await mcp.call("jc_datasource_check", { manifest: SOURCE_MANIFEST });
    expect((sourceCheck.structuredContent?.verdict as { ok?: boolean } | undefined)?.ok, said(sourceCheck)).toBe(true);
    const sourceChange = changeOf(await mcp.call("jc_resource_propose", { manifest: SOURCE_MANIFEST }));
    open.add(sourceChange);

    // AG-11, PF-58: a workload proposes, a person approves; called anyway, the door says why.
    const approval = await mcp.call("jc_change_approve", { change: sourceChange });
    expect(approval.isError, said(approval)).toBe(true);
    expect(said(approval)).toContain("an agent never approves or rejects a change");

    await approve(approver.page, PROJECT, sourceChange);
    open.delete(sourceChange);
    await expect
      .poll(() => listed(base, token, "datasources", SOURCE), { timeout: 240_000, intervals: [3_000] })
      .toBe(true);

    // The pipeline: its mapping tested on the feed itself (PL-49), proposed, approved by the person.
    const mapping = await mcp.call("jc_pipeline_test", { pipeline: PIPELINE_MANIFEST, sample: { url: FEED, format: "json" } });
    expect((mapping.structuredContent?.verdict as { ok?: boolean } | undefined)?.ok, said(mapping)).toBe(true);
    const pipelineChange = changeOf(await mcp.call("jc_resource_propose", { manifest: PIPELINE_MANIFEST }));
    open.add(pipelineChange);

    // AG-82: bringing a workspace back is a person's too, whatever the workspace.
    const bringBack = await mcp.call("jc_workspace_propose", { workspace: `t1595-${SUFFIX}` });
    expect(bringBack.isError, said(bringBack)).toBe(true);
    expect(said(bringBack)).toContain("an agent never brings a workspace back");

    await approve(approver.page, PROJECT, pipelineChange);
    open.delete(pipelineChange);
    await expect
      .poll(() => listed(base, token, "pipelines", PIPELINE), { timeout: 240_000, intervals: [3_000] })
      .toBe(true);
  } finally {
    for (const change of open) {
      await reject(approver.page, PROJECT, change);
    }
    await removeCompletely(steward, PROJECT, "pipelines", PIPELINE);
    await removeCompletely(steward, PROJECT, "datasources", SOURCE);
    await approver.context.close();
    await steward.context.close();
  }
});
