import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { manifestReadsAsRest } from "./manifestReads";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// The Scripts page (T-7084, PL-118…PL-121) on `vite preview` with the API answered from the
// browser, as approvals.spec.ts does: an author edits a stored script, runs its tests, sees the
// output that changed and proposes 1.2.0; a second person approves the Change; the author moves
// two pipelines' pins to the new version as one Change. The server side of each step (the digest
// check, the judge's `scripts` check, AG-11's self-approval refusal) has its own Rust tests.

const AUTHOR = { subject: "b7c1e0f4", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] };
const APPROVER = { subject: "c9d2e1a5", username: "jana", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-approver"] };
const OLD = "const r = bento.v0_msg_as_structured();\nr.x = r.e;\nbento.v0_msg_set_structured(r);\n";
const NEW = "const r = bento.v0_msg_as_structured();\nr.x = r.e + 1;\nbento.v0_msg_set_structured(r);\n";
const TESTS = [
  { name: "shifts-east", input: { e: 1 }, expected: { e: 1, x: 2 } },
  { name: "keeps-id", input: { e: 0, id: "a" }, expected: { e: 0, id: "a", x: 1 } },
];
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const version = (v: string, code: string) => ({ version: v, state: "published", code, sha256: "0".repeat(64), tests: TESTS });
const pipeline = (name: string, pinned: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: "helsinki" },
  spec: { steps: [{ kind: "script", scriptRef: { name: "sjtsk", version: pinned } }] },
});
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-script", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Script", name: "sjtsk", fields: 1 } },
  author: { name: AUTHOR.name, email: AUTHOR.email },
  createdAt: "2026-11-15T22:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
  planFields: [{ path: "spec.versions[2]", from: "", to: "1.2.0" }],
};

interface Stub {
  who: typeof AUTHOR;
  kinds: string[];
  scripts: boolean;
  runner: "answers" | "refuses";
  approved: boolean;
  sent: { method: string; path: string; body: string }[];
}

async function stubApi(page: Page, given: Partial<Stub> = {}): Promise<Stub> {
  const state: Stub = { who: AUTHOR, kinds: ["Script", "Pipeline"], scripts: true, runner: "answers", approved: false, sent: [], ...given };
  await page.route(/\/api\/v1\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const body = request.postData() ?? "";
    state.sent.push({ method: request.method(), path: `${path}${url.search}`, body });
    const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(value) });
    const script = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Script",
      metadata: { name: "sjtsk", namespace: "helsinki", title: "S-JTSK to WGS 84" },
      spec: {
        language: "javascript",
        versions: [version("1.0.0", OLD), version("1.1.0", OLD), ...(state.approved ? [version("1.2.0", NEW)] : [])],
      },
      status: { phase: "Ready" },
    };
    if (path.endsWith("/auth/me")) return json(state.who);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/admission")) return json({ admitted: true });
    if (path.endsWith("/permissions/me")) {
      const verbs = state.who === APPROVER ? ["approve"] : ["propose"];
      return json({ project: "helsinki", bootstrap: false, grants: [{ role: "pipeline-editor", binding: "editors", rule: { kinds: state.kinds, verbs } }] });
    }
    if (path.endsWith("JoinedContext.Portal.jc_script_test")) {
      if (state.runner === "refuses") {
        const message = "No runner is set up for project helsinki yet. Ask your administrator to configure the pipeline runner.";
        return json({ error: { code: "runner-unavailable", message, type: "https://joinedcontext.com/errors/runner-unavailable", title: "Service Unavailable", status: 503 } }, 503);
      }
      const asked = JSON.parse(body) as { code: string; tests: { name: string; input: { e: number; id?: string } }[] };
      const shift = asked.code.includes("r.e + 1") ? 1 : 0;
      return json({
        "@odata.context": "/api/v1/$metadata#Edm.Untyped",
        value: {
          results: asked.tests.map((t, i) => {
            const output = { ...t.input, x: t.input.e + shift };
            return { name: t.name, ok: JSON.stringify(output) === JSON.stringify(TESTS[i]?.expected), output, millis: 2 + i };
          }),
        },
      });
    }
    if (path.endsWith("/import")) {
      return url.searchParams.get("dryRun") === "All"
        ? json({ created: [], replaced: ["Pipeline/air-north", "Pipeline/air-south"], skipped: [], renamed: {}, native_files: 0, lane: "yellow", needs: [] })
        : json({ kind: "Change", metadata: { name: "chg-pins" }, spec: {}, status: { phase: "Pending" } }, 202);
    }
    if (path.endsWith("/changes/chg-script/approve")) {
      state.approved = true;
      return json({ ...CHANGE, kind: "Change", status: { ...CHANGE.status, phase: "Deploying" } }, 202);
    }
    if (path.endsWith("/changes/chg-script")) return json(CHANGE);
    if (path.endsWith("/changes")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: state.approved ? [] : [CHANGE] });
    if (path.endsWith("/scripts/sjtsk")) {
      if (request.method() === "GET") return json(script);
      return url.searchParams.get("dryRun") === "All"
        ? json({ valid: true, verdict: { ok: true, findings: [] } })
        : json({ kind: "Change", metadata: { name: "chg-script" }, spec: {}, status: { phase: "Pending" } });
    }
    if (path.endsWith("/scripts")) return json(list(state.scripts ? [script] : []));
    if (path.endsWith("/pipelines")) return json(list([pipeline("air-north", "1.0.0"), pipeline("air-south", "1.1.0"), pipeline("traffic", "1.0.0")]));
    return json(list([]));
  });
  return state;
}

const shot = (page: Page, name: string) => page.screenshot({ path: test.info().outputPath(name), fullPage: true });

async function bothWidths(page: Page, name: string) {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await shot(page, `${name}-1440.png`);
  await page.setViewportSize({ width: 400, height: 900 });
  const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(sideways, `${name} scrolls sideways at 400 px`).toBe(0);
  await shot(page, `${name}-400.png`);
  await page.setViewportSize({ width: 1440, height: 1000 });
}

async function editAndRun(page: Page) {
  await page.getByRole("combobox", { name: "Script", exact: true }).selectOption("sjtsk");
  // Monaco's own input is a zero-size edit context: a person clicks the lines.
  await page.locator(".monaco-editor .view-lines").click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  // Monaco closes brackets as one types; the new code has none to close.
  await page.keyboard.insertText(NEW);
  await page.getByRole("button", { name: "Run tests" }).click();
}

test.describe("scripts", () => {
  test("edit, test, propose 1.2.0, a second person approves, two pipelines move to it", async ({ page }) => {
    const state = await stubApi(page);
    await page.goto("/projects/helsinki/scripts?lang=en");
    await expect(page.getByRole("heading", { name: "Script workbench" })).toBeVisible();

    await editAndRun(page);
    const results = page.getByRole("table", { name: "Test results" });
    await expect(results.getByText("Changed").first()).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Version" })).toHaveValue("1.2.0");
    await bothWidths(page, "scripts-results");

    await page.getByRole("button", { name: "Propose version 1.2.0" }).click();
    await expect(page.getByText(/Version 1\.2\.0 is proposed/)).toBeVisible();
    const proposed = state.sent.filter((s) => s.path === "/api/v1/projects/helsinki/scripts/sjtsk" && s.method !== "GET");
    expect(proposed).toHaveLength(1);
    expect(JSON.parse(proposed[0].body).spec.versions.map((v: { version: string }) => v.version)).toEqual(["1.0.0", "1.1.0", "1.2.0"]);
    await bothWidths(page, "scripts-proposed");

    // A second person approves the Change on the Approvals page.
    state.who = APPROVER;
    await page.goto("/projects/helsinki/approvals/chg-script?lang=en");
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByText("Deploying")).toBeVisible();
    expect(state.approved).toBe(true);

    state.who = AUTHOR;
    await page.goto("/projects/helsinki/scripts?lang=en");
    await page.getByRole("combobox", { name: "Script", exact: true }).selectOption("sjtsk");
    const used = page.getByRole("region", { name: "Pipelines using sjtsk" }).or(page.locator('[aria-labelledby="script-used-title"]'));
    await expect(used.getByRole("combobox", { name: "Move pins to" })).toHaveValue("1.2.0");
    await used.getByRole("checkbox", { name: "air-north" }).check();
    await used.getByRole("checkbox", { name: "air-south" }).check();
    await used.getByRole("button", { name: "Move 2 pipelines to 1.2.0" }).click();
    await expect(used.getByText("Proposed one Change moving air-north and air-south to 1.2.0.")).toBeVisible();
    const imports = state.sent.filter((s) => s.path.includes("/import"));
    expect(imports.map((s) => s.path.includes("dryRun=All"))).toEqual([true, false]);
    expect(imports[1].body).toContain('"version":"1.2.0"');
    expect(imports[1].body).not.toContain("traffic");
    await bothWidths(page, "scripts-moved");
  });

  test("a project with no script says how to start one", async ({ page }) => {
    await stubApi(page, { scripts: false });
    await page.goto("/projects/helsinki/scripts?lang=en");
    await expect(page.getByRole("heading", { name: "Script workbench" })).toBeVisible();
    await bothWidths(page, "scripts-empty");
  });

  test("a runner that cannot run says why", async ({ page }) => {
    await stubApi(page, { runner: "refuses" });
    await page.goto("/projects/helsinki/scripts?lang=en");
    await editAndRun(page);
    await expect(page.getByText(/No runner is set up for project helsinki yet/)).toBeVisible();
    await bothWidths(page, "scripts-error");
  });

  test("a role that proposes no script is told so", async ({ page }) => {
    await stubApi(page, { kinds: ["Pipeline"] });
    await page.goto("/projects/helsinki/scripts?lang=en");
    await expect(page.getByText(/^Your role does not propose scripts in this project, so it cannot run their tests either/).first()).toBeVisible();
    await bothWidths(page, "scripts-refusal");
  });
});
