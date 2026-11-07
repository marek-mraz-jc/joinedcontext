/**
 * Load from a REST-only client, curl's way, on dev (T-1596, PF-57, PF-50, PF-58): the seeded account
 * `helsinki/pipeline-proposer` holds a `portal-api` token (T-2245) and nothing else, dry-runs a
 * DataSource, is refused the unchecked proposal, checks it, proposes it, is refused its approval, and
 * a second identity, a person in the Portal, approves; the source is then listed, Live.
 *
 * Every refusal on the way says in its `detail` what to do: a problem document (PF-50), or the
 * verdict gate's own document (PF-57, T-0956). The journey removes what it made and rejects a
 * change it left open, so dev keeps nothing of it.
 */
import { expect, test } from "@playwright/test";
import { FEED } from "./loadFeed";
import { APPROVER, STEWARD, approve, reject, removeCompletely, serviceAccountToken, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
// Journey-named (`t1596-…`), so the residue sweep's janitor may remove what a failed run leaves.
const SOURCE = `t1596-rest-source-${SUFFIX}`;

const MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name: SOURCE, namespace: PROJECT },
  spec: { type: "http", http: { url: FEED, verb: "GET", timeout: "15s" } },
};

interface Problem {
  type?: string;
  title?: string;
  status?: number;
  detail?: string;
  error?: string;
  check?: string;
}

test("a REST-only client dry-runs, checks and proposes a data source, is refused what it may not do with a problem it can act on, and a person approves it", async ({ browser, baseURL }) => {
  test.setTimeout(600_000);
  const base = baseURL ?? "";
  const token = await serviceAccountToken(base);
  // Plain fetch with the bearer alone: no cookie, no CSRF header, no journey header.
  const send = async (method: string, path: string, body?: unknown) => {
    const answer = await fetch(new URL(path, base), {
      method,
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json", accept: "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await answer.text();
    return { status: answer.status, type: answer.headers.get("content-type") ?? "", text, json: () => JSON.parse(text) as Record<string, unknown> };
  };
  /**
   * A refusal a person can act on: its status and a detail that says `names`, as problem+json;
   * the verdict gate answers its own document instead, the body every door shares (T-0956).
   */
  const refused = (answer: Awaited<ReturnType<typeof send>>, status: number, names: string, media = "application/problem+json"): Problem => {
    expect(answer.status, answer.text.slice(0, 500)).toBe(status);
    expect(answer.type).toContain(media);
    const problem = answer.json() as Problem;
    expect(problem.detail ?? "", answer.text.slice(0, 500)).toContain(names);
    return problem;
  };
  const collection = `/api/v1/projects/${PROJECT}/datasources`;
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}?lang=en`);
  let change = "";

  try {
    // PF-50: a kind the account's role does not name is refused before the manifest is read.
    const endpoint = await send("POST", `/api/v1/projects/${PROJECT}/endpoints`, {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: `t1596-rest-endpoint-${SUFFIX}`, namespace: PROJECT },
      spec: {},
    });
    refused(endpoint, 403, "no role grants propose on Endpoint");

    // The dry run: the plan, nothing written.
    const dryRun = await send("POST", `${collection}?dryRun=true`, MANIFEST);
    expect(dryRun.status, dryRun.text.slice(0, 500)).toBe(200);
    expect(dryRun.json().valid, dryRun.text.slice(0, 500)).toBe(true);

    // PF-57: proposed before its check, the proposal names the check to run.
    const unchecked = refused(await send("POST", collection, MANIFEST), 409, "check it", "application/json");
    expect(unchecked.error).toBe("verdict_required");
    expect(unchecked.check).toBe("jc_datasource_check");

    const check = await send("POST", `/api/v1/projects/${PROJECT}/ops/jc_datasource_check`, { manifest: MANIFEST });
    expect(check.status, check.text.slice(0, 500)).toBe(200);
    expect((check.json().verdict as { ok?: boolean } | undefined)?.ok, check.text.slice(0, 500)).toBe(true);

    const proposed = await send("POST", collection, MANIFEST);
    expect(proposed.status, proposed.text.slice(0, 500)).toBe(202);
    change = ((proposed.json().metadata as { name?: string } | undefined)?.name ?? "").trim();
    expect(change, proposed.text.slice(0, 500)).toMatch(/^chg-[0-9a-f]{8}$/);

    // PF-58: a workload never approves, not even its own change over the REST route.
    refused(await send("POST", `/api/v1/projects/${PROJECT}/changes/${change}/approve`), 403, "no role grants approve");

    // The second identity: a person approves in the Portal, and the source is Live.
    await approve(approver.page, PROJECT, change);
    change = "";
    await expect
      .poll(
        async () => {
          const one = await send("GET", `${collection}/${SOURCE}`);
          return one.status;
        },
        { timeout: 240_000, intervals: [3_000] },
      )
      .toBe(200);
  } finally {
    if (change) {
      await reject(approver.page, PROJECT, change);
    }
    await removeCompletely(steward, PROJECT, "datasources", SOURCE);
    await approver.context.close();
    await steward.context.close();
  }
});
