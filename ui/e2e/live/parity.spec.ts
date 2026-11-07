/**
 * The same change through every door gives the same Change (T-0743, AG-77, ADR-N-021, CC-34):
 * for one kind of each family, the steward makes one update four ways, the form's YAML view,
 * the REST route, the operations registry and the assistant, and the four plans the approvals
 * route answers are compared field by field. The approver rejects every proposal, so dev keeps
 * what it had. On demand against dev, never in the ETSI suite.
 *
 * ponytail: the registry leg calls `/ops/jc_resource_propose` with the session, the function the
 * MCP `tools/call` reaches behind a bearer token; the assistant leg runs for the two kinds whose
 * prompts change.spec.ts already proves, a model round trip per kind is the ceiling.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Locator, Page } from "@playwright/test";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { APPROVER, STEWARD, checkManifest, proposedChange, reject, signIn } from "./portal";

const PROJECT = "helsinki";
/** Organization-scoped kinds are served under the `org` project (PF-49). */
const ORG = "org";

type Manifest = Record<string, unknown> & { kind: string; metadata: { name: string }; spec: Record<string, unknown> };

interface Case {
  kind: string;
  plural: string;
  /** The project the resource API serves the kind under. */
  project: string;
  /** The page that offers the form. */
  route: string;
  name: string;
  /**
   * The form door: the accessible name of the control that opens the kind's editor, on the
   * resource's row when `row` is set. Absent when the page edits something other than the
   * manifest (the models page edits the model's source, `PUT …/source`, an operation of its own).
   */
  form?: { edit: (manifest: Manifest) => string; row?: boolean };
  /** The one update, applied to a copy of the stored manifest. */
  change: (manifest: Manifest) => void;
  /** What to ask the assistant for the same update, when its prompt is known to land. */
  ask?: { text: string; url: RegExp };
  /**
   * Who withdraws the proposal: the approver, or the steward for the access kinds, which the
   * seeded approver role does not approve and the steward administers (org-admin, PF-58).
   */
  decidedBy?: "steward";
}

const CASES: Case[] = [
  {
    kind: "Endpoint",
    plural: "endpoints",
    project: PROJECT,
    route: `/projects/${PROJECT}/endpoints`,
    name: "helsinki-bikes",
    form: { edit: () => "Edit", row: true },
    change: (m) => {
      const limits = (m.spec.rateLimits ?? {}) as Record<string, unknown>;
      m.spec.rateLimits = { ...limits, requestsPerMinute: 300 };
    },
    ask: {
      text: "Set the helsinki-bikes endpoint's rate limit to 300 requests per minute",
      url: /\/projects\/helsinki\/endpoints/,
    },
  },
  {
    kind: "Pipeline",
    plural: "pipelines",
    project: PROJECT,
    route: `/projects/${PROJECT}/pipelines`,
    name: "hel-news",
    form: { edit: () => "Edit", row: true },
    change: (m) => {
      m.spec.enabled = false;
    },
    ask: { text: "Pause the hel-news pipeline", url: /\/projects\/helsinki\/pipelines\?edit=hel-news/ },
  },
  {
    kind: "DataModel",
    plural: "datamodels",
    project: PROJECT,
    route: `/projects/${PROJECT}/models`,
    name: "helsinki",
    change: (m) => {
      const title = ((m.metadata as Record<string, unknown>).title ?? {}) as Record<string, string>;
      (m.metadata as Record<string, unknown>).title = { ...title, en: "Helsinki city context (parity)" };
    },
  },
  {
    kind: "RoleBinding",
    plural: "rolebindings",
    project: ORG,
    route: `/projects/${PROJECT}/settings/members`,
    name: "stewards",
    form: {
      // The access page labels a binding by whom it binds and to what (RoleBindings.tsx).
      edit: (m) =>
        `Edit ${((m.spec.subjects ?? []) as { user?: string; group?: string }[])
          .map((subject) => subject.user ?? `Group ${subject.group ?? ""}`)
          .join(", ")}: ${String(m.spec.role ?? "")}`,
    },
    change: (m) => {
      m.spec.validity = { notAfter: "2031-12-31T23:59:59Z" };
    },
    decidedBy: "steward",
  },
];

test.setTimeout(900_000);
// Two signed-in contexts and Monaco are enough for the sandbox's memory; a trace is not.
test.use({ trace: "off" });

/** The ids of the changes still pending for one resource, so a run aborted earlier leaves no residue. */
async function pendingFor(page: Page, c: Case): Promise<string[]> {
  const answer = await page.request.get(`/api/v1/projects/${c.project}/changes`);
  expect(answer.ok(), `list changes of ${c.project}`).toBe(true);
  const body = (await answer.json()) as {
    items?: { metadata: { name: string }; status: { phase: string }; summary: { params: { kind?: string; name?: string } } }[];
  };
  return (body.items ?? [])
    .filter((item) => item.status.phase === "PendingApproval")
    .filter((item) => item.summary.params.kind === c.kind && item.summary.params.name === c.name)
    .map((item) => item.metadata.name);
}

/** The double-submit token the Portal's write routes want beside the session cookie. */
async function csrf(context: BrowserContext): Promise<Record<string, string>> {
  const token = (await context.cookies()).find((cookie) => cookie.name === "__Host-jc_csrf")?.value ?? "";
  expect(token, "the session carries a CSRF cookie").not.toBe("");
  return { "x-csrf-token": token };
}

/** The stored manifest without its status, the way every door takes it back. */
async function stored(page: Page, c: Case): Promise<Manifest> {
  const answer = await page.request.get(`/api/v1/projects/${c.project}/${c.plural}/${c.name}`);
  expect(answer.ok(), `get ${c.plural}/${c.name}`).toBe(true);
  const manifest = (await answer.json()) as Manifest & { status?: unknown };
  delete manifest.status;
  return manifest;
}

function changed(c: Case, manifest: Manifest): Manifest {
  const copy = JSON.parse(JSON.stringify(manifest)) as Manifest;
  c.change(copy);
  return copy;
}

/** The planned fields of a change, sorted, so two plans compare as text. */
async function planned(page: Page, project: string, change: string): Promise<string> {
  const detail = await page.request.get(`/api/v1/projects/${project}/changes/${change}`);
  expect(detail.ok(), `change ${change}`).toBe(true);
  const fields = (((await detail.json()) as { planFields?: { path: string; from?: unknown; to?: unknown }[] })
    .planFields ?? []).slice();
  fields.sort((a, b) => a.path.localeCompare(b.path));
  return JSON.stringify(fields);
}

async function viaRest(page: Page, context: BrowserContext, c: Case, manifest: Manifest): Promise<string> {
  await checkManifest(page, context, c.project, manifest);
  const answer = await page.request.put(`/api/v1/projects/${c.project}/${c.plural}/${c.name}`, {
    headers: { ...(await csrf(context)), "content-type": "application/json" },
    data: manifest,
  });
  expect(answer.status(), `REST put ${c.plural}/${c.name}: ${await answer.text()}`).toBe(202);
  return ((await answer.json()) as { metadata: { name: string } }).metadata.name;
}

async function viaRegistry(page: Page, context: BrowserContext, c: Case, manifest: Manifest): Promise<string> {
  await checkManifest(page, context, c.project, manifest);
  const answer = await page.request.post(`/api/v1/projects/${c.project}/ops/jc_resource_propose`, {
    headers: { ...(await csrf(context)), "content-type": "application/json" },
    data: { manifest },
  });
  expect(answer.status(), `registry propose ${c.kind}/${c.name}: ${await answer.text()}`).toBe(202);
  return ((await answer.json()) as { changeId: string }).changeId;
}

/**
 * Replaces what Monaco shows with `text`, the way a person selects all and pastes. Pasted, not
 * typed: Monaco indents every typed newline after the line above, which bends a typed manifest.
 */
async function replaceYaml(dialog: Locator, text: string): Promise<void> {
  const editor = dialog.locator(".monaco-editor").first();
  await expect(editor).toBeVisible({ timeout: 60_000 });
  const page = dialog.page();
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.evaluate((clip) => navigator.clipboard.writeText(clip), text);
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("ControlOrMeta+v");
}

/** Proposes from a form, running its check first when the form asks for a fresh one. */
async function proposeFrom(form: Locator): Promise<void> {
  const propose = form.getByRole("button", { name: "Propose change" });
  if (await propose.isDisabled()) {
    const check = form.getByRole("button", { name: "Check", exact: true });
    if (await check.count()) {
      await check.click();
    }
  }
  // The dialog's own words when the proposal never opens: the verdict, a YAML error, a refusal.
  await expect(propose, await form.innerText()).toBeEnabled({ timeout: 120_000 });
  await propose.click();
}

/** The kind's own editor, its YAML view when it has one, the manifest typed, Propose. */
async function viaForm(page: Page, c: Case, manifest: Manifest): Promise<string> {
  await page.goto(`${c.route}?lang=en`, { waitUntil: "load" });
  const main = page.locator("main");
  const scope = c.form!.row ? main.locator("tr").filter({ hasText: c.name }).first() : main;
  await scope.getByRole("button", { name: c.form!.edit(manifest), exact: true }).first().click();
  const dialog = page.getByTestId("form-page");
  await expect(dialog).toBeVisible();
  const yamlTab = dialog.getByRole("tab", { name: "YAML" });
  if (await yamlTab.count()) {
    await yamlTab.click();
  }
  await replaceYaml(dialog, stringifyYaml(manifest));
  // The dialog's own Check runs on the YAML typed (T-0884), then Propose.
  await proposeFrom(dialog);
  const inDialog = dialog.getByText(/^chg-[0-9a-f]{8}$/);
  if (await inDialog.count()) {
    return (await inDialog.textContent()) ?? "";
  }
  return proposedChange(page);
}

/** Asks in the dock: its first composer, or the conversation's once one is running. */
async function ask(page: Page, text: string): Promise<void> {
  const bubble = page.getByRole("button", { name: "Open the assistant" });
  if (await bubble.count()) {
    await bubble.first().click();
  }
  const composer = page.getByLabel(/^(Ask the assistant|Tell the assistant what to build or change…)$/).first();
  await composer.fill(text);
  await composer.press("Enter");
}

async function viaAssistant(page: Page, c: Case): Promise<string> {
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await ask(page, c.ask!.text);
  await expect(page).toHaveURL(c.ask!.url, { timeout: 180_000 });
  const form = page.getByTestId("form-page");
  await expect(form).toBeVisible();
  await proposeFrom(form);
  return proposedChange(page);
}

for (const c of CASES) {
  test(`${c.kind} ${c.name}: the form, the REST route, the registry and the assistant plan the same change`, async ({
    browser,
  }) => {
    const steward = await signIn(browser, STEWARD, `${c.route}?lang=en`);
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    const decider = c.decidedBy === "steward" ? steward.page : approver.page;
    const page = steward.page;
    for (const left of await pendingFor(page, c)) {
      await reject(decider, c.project, left);
    }
    const manifest = changed(c, await stored(page, c));
    expect(parseYaml(stringifyYaml(manifest))).toEqual(manifest);

    // One proposal at a time: a resource with an open change refuses a second one, so every
    // door's plan is read and its proposal withdrawn before the next door opens.
    const doors: [string, () => Promise<string>][] = [
      ["rest", () => viaRest(page, steward.context, c, manifest)],
      ["registry", () => viaRegistry(page, steward.context, c, manifest)],
    ];
    if (c.form) {
      doors.push(["form", () => viaForm(page, c, manifest)]);
    }
    if (c.ask) {
      doors.push(["assistant", () => viaAssistant(page, c)]);
    }
    const plans: Record<string, string> = {};
    try {
      for (const [door, propose] of doors) {
        const id = await propose();
        try {
          plans[door] = await planned(page, c.project, id);
        } finally {
          // Withdrawn whatever happened above, so dev keeps what it had.
          await reject(decider, c.project, id);
        }
      }
      test.info().annotations.push({ type: "plan", description: plans.rest });
      for (const [door, plan] of Object.entries(plans)) {
        expect(plan, `${door} plans what the REST route plans`).toBe(plans.rest);
      }
      expect(plans.rest, "the plan holds the change").not.toBe("[]");
    } finally {
      await steward.context.close();
      await approver.context.close();
    }
  });
}
