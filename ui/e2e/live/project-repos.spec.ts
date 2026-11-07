/**
 * T-2647 — one repository per project, proved on dev (PF-85…PF-89, CC-86…CC-90, MF-45, MF-46,
 * AG-86). Six steps, run in order, each printing what it measured into the test's annotations;
 * nothing is asserted from a screenshot.
 *
 * 1. A person's own forge token reads a project repository exactly when the Portal lets them read
 *    the project, and the organization repository exactly when they hold a binding at the
 *    organization (PF-87). On dev the organization's projects are `visibility: organization`, so
 *    every signed-in person reads every project and the 404 half is measured only for a project
 *    the person cannot read; the step names which case it met.
 * 2. The steward pushes a branch of `helsinki-mobility` with git (PF-87): a parameter declared in
 *    `project.yaml` and used as `{param:…}` in an Endpoint's description. The Portal lists the
 *    merge request as a Change (T-3433); approved, the registry entry gets a value (T-3432), and
 *    the gateway's document of the Endpoint shows that value (CC-86, CC-88).
 * 3. A tag `v0.2.0` pinned in the registry entry: a later merge to `main` changes nothing the
 *    gateway serves until the pin moves back (PF-86, CC-90).
 * 4. Duplicate with another parameter value (PF-89): the copy's Endpoint slugs are its own, the
 *    origin's Endpoints are unchanged, and each serves its own value.
 * 5. `helsinki` exported as git with its three applications, imported under a new slug with the
 *    parameter form (MF-45, MF-46): every repository's head equal, and the applications build from
 *    the new repositories and open.
 * 6. An app-builder run in the imported project reaches its repositories only (AG-86), measured on
 *    the forge's activity feeds. A live model run is paid: it runs only with JC_LIVE_MODEL_RUN=1
 *    (owner, 2026-10-07, 6 USD spend guard).
 *
 * dev is left as it was: the parameter, the description and the pin are put back by Changes, the
 * copy and the import are deleted (PF-77; their names stay reserved for the cooldown, so each run
 * takes names of its own), and the forge tokens are revoked.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page, TestInfo } from "@playwright/test";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { parse, stringify } from "yaml";
import { FORGE_ORG, ORG_REPO, cloneRepository, forgeApi, forgeSignIn, forgeSignOut, git, gitReadStatus } from "./forge";
import type { ForgePerson } from "./forge";
import { STEWARD, VIEWER, approve, csrf, signIn } from "./portal";

const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const MOBILITY = "helsinki-mobility";
const HELSINKI = "helsinki";
const COPY = `hm-copy-${SUFFIX}`;
const IMPORTED = `helsinki-2-${SUFFIX}`;
const PARAMETER = "stationsNote";
const VALUE = `Stations as measured by T-2647 ${SUFFIX}`;
const COPY_VALUE = `Stations of the copy ${SUFFIX}`;
const TAG = "v0.2.0";
const AFTER_PIN = " (merged after the pin)";

test.describe.configure({ mode: "serial" });
test.setTimeout(1_800_000);

type Session = { context: BrowserContext; page: Page };
let steward: Session | null = null;
let forgeSteward: ForgePerson | null = null;
/** What step 2 changed, so the clean-up can put it back. */
let endpointFile = "";
let endpointSlug = "";
let originalDescription: unknown = undefined;
let originalRegistry: { ref: string; parameters: Record<string, unknown> } | null = null;

function measured(info: TestInfo, what: string, value: unknown): void {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  info.annotations.push({ type: "measured", description: `${what}: ${text}` });
  console.log(`[T-2647] ${what}: ${text}`);
}

async function stewardSession(browser: Parameters<typeof signIn>[0]): Promise<Session> {
  steward ??= await signIn(browser, STEWARD, `/projects/${MOBILITY}/settings/general?lang=en`);
  return steward;
}

async function json<T>(answer: { ok(): boolean; status(): number; text(): Promise<string> }, what: string): Promise<T> {
  const text = await answer.text();
  if (!answer.ok()) throw new Error(`${what}: ${answer.status()} ${text}`);
  return JSON.parse(text) as T;
}

/**
 * Approves a red-lane change, typing back the name the approval page asks for (CC-19). A merge
 * request of the organization repository (`chg-org-…`: a registry entry, a new or deleted
 * project) is the organization's Change and is approved on its page (CC-87).
 */
async function approveTyped(page: Page, at: string, change: string): Promise<void> {
  const project = change.startsWith("chg-org-") ? "org" : at;
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const input = page.locator("#confirm-resource-name");
  const button = page.getByRole("button", { name: "Approve", exact: true });
  await expect(button.and(page.locator(":enabled")).or(input).first()).toBeVisible({ timeout: 90_000 });
  const name = (await input.count()) ? ((await input.getAttribute("placeholder")) ?? "") : undefined;
  await approve(page, project, change, name);
}

/** The registry entry of a project, read the way the Release section reads it (T-3432). */
async function registry(page: Page, project: string): Promise<{ ref: string; parameters: Record<string, unknown>; tags: { name: string }[] }> {
  return json(await page.request.get(`/api/v1/projects/${project}/registry`), `registry of ${project}`);
}

/** Proposes a repoint and approves it as the organization administrator (PF-58), answering the change. */
async function repoint(session: Session, project: string, body: { ref?: string; parameters?: Record<string, unknown> }): Promise<string> {
  const answer = await session.page.request.put(`/api/v1/projects/${project}/registry`, {
    headers: { "x-csrf-token": await csrf(session.context) },
    data: body,
  });
  const change = (await json<{ metadata: { name: string } }>(answer, `repoint ${project}`)).metadata.name;
  await approveTyped(session.page, project, change);
  return change;
}

/** What the gateway's document of one Endpoint says it is (EP-27): the rendered description. */
async function servedDescription(page: Page, slug: string): Promise<string> {
  const doc = await json<{ info?: { description?: string } }>(
    await page.request.get(`/api/endpoint/${slug}/openapi.json`),
    `the gateway's document of ${slug}`,
  );
  return doc.info?.description ?? "";
}

/** The Endpoints of a project, name to slug. */
async function endpointSlugs(page: Page, project: string): Promise<Record<string, string>> {
  const list = await json<{ items: { metadata: { name: string }; spec: { slug?: string } }[] }>(
    await page.request.get(`/api/v1/projects/${project}/endpoints`),
    `the Endpoints of ${project}`,
  );
  return Object.fromEntries(list.items.map((item) => [item.metadata.name, item.spec.slug ?? ""]));
}

/**
 * Pushes `edit` on a branch of the project repository, opens its merge request in the forge and
 * approves the Change the Portal lists for it (T-3433): the way a writer's own git work lands.
 */
async function landWithGit(
  session: Session,
  person: ForgePerson,
  branch: string,
  message: string,
  edit: (dir: string) => Promise<void>,
): Promise<{ change: string; number: number }> {
  const clone = await cloneRepository(person, MOBILITY);
  try {
    await git(person, clone.dir, ["checkout", "--quiet", "-b", branch]);
    await edit(clone.dir);
    await git(person, clone.dir, ["commit", "--quiet", "-am", message]);
    await git(person, clone.dir, ["push", "--quiet", "origin", branch]);
  } finally {
    await clone.done();
  }
  const opened = await json<{ number: number }>(
    await forgeApi(person, "POST", `/repos/${FORGE_ORG}/${MOBILITY}/pulls`, { head: branch, base: "main", title: message }),
    `open the merge request of ${branch}`,
  );
  // A merge request of the project repository is the Change `chg-` and its number (CC-87, T-3433).
  const change = `chg-${opened.number.toString(16).padStart(8, "0")}`;
  await expect
    .poll(async () => (await session.page.request.get(`/api/v1/projects/${MOBILITY}/changes/${change}`)).status(), { timeout: 120_000 })
    .toBe(200);
  await approveTyped(session.page, MOBILITY, change);
  return { change, number: opened.number };
}

/** The parts of a manifest the journey reads and edits. */
interface Manifest {
  kind?: string;
  metadata: { description?: unknown } & Record<string, unknown>;
  spec: { parameters?: Record<string, unknown>; slug?: string } & Record<string, unknown>;
}

async function editYaml(file: string, change: (document: Manifest) => void): Promise<void> {
  const document = parse(await readFile(file, "utf8")) as Manifest;
  change(document);
  await writeFile(file, stringify(document));
}

/** Deletes a project the journey made, through its red-lane Change (PF-77). */
async function deleteProject(session: Session, project: string): Promise<void> {
  const answer = await session.page.request.delete(`/api/v1/projects/${project}`, {
    headers: { "x-csrf-token": await csrf(session.context) },
  });
  if (answer.status() === 404) return;
  const change = (await json<{ metadata: { name: string } }>(answer, `delete ${project}`)).metadata.name;
  await approveTyped(session.page, project, change);
}

test.afterAll(async ({ browser }) => {
  const session = await stewardSession(browser);
  const failures: string[] = [];
  const attempt = async (what: string, step: () => Promise<unknown>) => {
    try {
      await step();
    } catch (error) {
      failures.push(`${what}: ${String(error)}`);
    }
  };
  if (originalRegistry) {
    const original = originalRegistry;
    await attempt("put the registry entry back", async () => {
      const now = await registry(session.page, MOBILITY);
      if (now.ref !== original.ref || JSON.stringify(now.parameters) !== JSON.stringify(original.parameters)) {
        await repoint(session, MOBILITY, { ref: original.ref, parameters: original.parameters });
      }
    });
  }
  if (forgeSteward && endpointFile) {
    const person = forgeSteward;
    await attempt("put the Endpoint and project.yaml back", () =>
      landWithGit(session, person, `t2647-restore-${SUFFIX}`, `T-2647: put ${MOBILITY} back as it was`, async (dir) => {
        await editYaml(join(dir, endpointFile), (document) => {
          if (originalDescription === undefined) delete document.metadata.description;
          else document.metadata.description = originalDescription;
        });
        await editYaml(join(dir, "project.yaml"), (document) => {
          delete document.spec.parameters?.[PARAMETER];
          if (document.spec.parameters && Object.keys(document.spec.parameters).length === 0) delete document.spec.parameters;
        });
      }),
    );
  }
  await attempt(`delete ${COPY}`, () => deleteProject(session, COPY));
  await attempt(`delete ${IMPORTED}`, () => deleteProject(session, IMPORTED));
  await forgeSignOut(forgeSteward);
  await session.context.close();
  expect(failures, "the journey left dev as it found it").toEqual([]);
});

test("1. a person's forge token reads exactly the repositories the Portal lets them read (PF-87)", async ({ browser }, info) => {
  const viewer = await forgeSignIn(browser, VIEWER, `t2647-viewer-${SUFFIX}`);
  const portal = await signIn(browser, VIEWER, `/projects?lang=en`);
  try {
    const listed = await json<{ items: { name: string }[] }>(await portal.page.request.get("/api/v1/projects"), "projects");
    const candidates = [MOBILITY, HELSINKI, ...listed.items.map((item) => item.name)];
    let boundaryMet = false;
    for (const project of new Set(candidates)) {
      const readsInPortal = (await portal.page.request.get(`/api/v1/projects/${project}`)).ok();
      const status = await gitReadStatus(viewer, project);
      measured(info, `${VIEWER.user} on ${project}: Portal ${readsInPortal ? "reads" : "404"}, forge ${status}`, "");
      expect(status, `the forge answers ${project} as the Portal does`).toBe(readsInPortal ? 200 : 404);
      boundaryMet ||= !readsInPortal;
    }
    const atOrganization = await json<{ grants?: unknown[] }>(
      await portal.page.request.get("/api/v1/projects/org/permissions/me"),
      "the viewer's organization permissions",
    );
    const orgStatus = await gitReadStatus(viewer, ORG_REPO);
    measured(info, `${VIEWER.user} on the organization repository`, `${(atOrganization.grants ?? []).length} organization grants, forge ${orgStatus}`);
    expect(orgStatus, "the organization repository answers as the organization binding says").toBe(
      (atOrganization.grants ?? []).length > 0 ? 200 : 404,
    );
    measured(info, "the 404 half of PF-87", boundaryMet ? "measured on a project the viewer cannot read" : "not reachable on this installation: the viewer reads every project");

    const clone = await cloneRepository(viewer, MOBILITY);
    try {
      const head = (await git(viewer, clone.dir, ["rev-parse", "HEAD"])).trim();
      await readFile(join(clone.dir, "project.yaml"), "utf8");
      measured(info, `${VIEWER.user} cloned ${MOBILITY} with their own token`, head);
    } finally {
      await clone.done();
    }
  } finally {
    await forgeSignOut(viewer);
    await portal.context.close();
  }
});

test("2. a branch the steward pushes with git lands as a Change, and the gateway serves its parameter (PF-87, CC-88)", async ({ browser }, info) => {
  const session = await stewardSession(browser);
  forgeSteward = await forgeSignIn(browser, STEWARD, `t2647-steward-${SUFFIX}`);
  originalRegistry = await registry(session.page, MOBILITY);
  measured(info, `registry entry of ${MOBILITY} before`, originalRegistry);

  // The Endpoint whose description will carry the parameter: the first one the repository holds.
  const probe = await cloneRepository(forgeSteward, MOBILITY);
  try {
    const files = (await git(forgeSteward, probe.dir, ["ls-files", "*.yaml"])).split("\n").filter(Boolean);
    for (const file of files) {
      const document = parse(await readFile(join(probe.dir, file), "utf8")) as Manifest | null;
      if (document?.kind === "Endpoint" && document.spec?.slug) {
        endpointFile = file;
        endpointSlug = document.spec.slug;
        originalDescription = document.metadata?.description;
        break;
      }
    }
  } finally {
    await probe.done();
  }
  expect(endpointFile, `${MOBILITY} holds an Endpoint`).not.toBe("");
  measured(info, "Endpoint used", `${endpointFile} (${endpointSlug})`);

  const landed = await landWithGit(session, forgeSteward, `t2647-${SUFFIX}`, `T-2647: ${PARAMETER} as a parameter`, async (dir) => {
    await editYaml(join(dir, "project.yaml"), (document) => {
      document.spec.parameters = {
        ...(document.spec.parameters ?? {}),
        [PARAMETER]: { type: "string", default: "City bike stations", description: "What the stations Endpoint says it is" },
      };
    });
    await editYaml(join(dir, endpointFile), (document) => {
      document.metadata.description = { en: `{param:${PARAMETER}}` };
    });
  });
  measured(info, "Change of the pushed branch", `${landed.change} (merge request ${landed.number})`);

  const started = Date.now();
  await repoint(session, MOBILITY, { parameters: { ...originalRegistry.parameters, [PARAMETER]: VALUE } });
  await expect.poll(() => servedDescription(session.page, endpointSlug), { timeout: 600_000, intervals: [5_000] }).toBe(VALUE);
  measured(info, "registry value served by the gateway after", `${Math.round((Date.now() - started) / 1000)} s`);
});

test("3. a pinned tag holds what dev serves until the pin moves (PF-86, CC-90)", async ({ browser }, info) => {
  const session = await stewardSession(browser);
  const person = forgeSteward;
  if (!person) throw new Error("step 2 made no forge session");
  const tag = await forgeApi(person, "GET", `/repos/${FORGE_ORG}/${MOBILITY}/tags/${TAG}`);
  if (tag.status() === 404) {
    await json(await forgeApi(person, "POST", `/repos/${FORGE_ORG}/${MOBILITY}/tags`, { tag_name: TAG, target: "main", message: "T-2647" }), `tag ${TAG}`);
  }
  measured(info, `tag ${TAG}`, tag.status() === 404 ? "made on main" : "already there, reused");
  await repoint(session, MOBILITY, { ref: TAG });
  expect((await registry(session.page, MOBILITY)).ref).toBe(TAG);

  await landWithGit(session, person, `t2647-after-pin-${SUFFIX}`, "T-2647: a change after the pin", async (dir) => {
    await editYaml(join(dir, endpointFile), (document) => {
      document.metadata.description = { en: `{param:${PARAMETER}}${AFTER_PIN}` };
    });
  });
  // Two checkout intervals and a render past the merge: still the pinned text.
  const watched = Date.now();
  while (Date.now() - watched < 120_000) {
    expect(await servedDescription(session.page, endpointSlug), "the gateway still serves the pinned release").toBe(VALUE);
    await session.page.waitForTimeout(10_000);
  }
  measured(info, "served while pinned, 120 s after the merge", VALUE);

  const moved = Date.now();
  await repoint(session, MOBILITY, { ref: originalRegistry?.ref ?? "main" });
  await expect.poll(() => servedDescription(session.page, endpointSlug), { timeout: 600_000, intervals: [5_000] }).toBe(`${VALUE}${AFTER_PIN}`);
  measured(info, "the merge after the pin served once the pin moved back", `${Math.round((Date.now() - moved) / 1000)} s`);
});

test("4. a duplicate takes its own slugs and its own parameter value, and the origin is untouched (PF-89)", async ({ browser }, info) => {
  const session = await stewardSession(browser);
  const before = await endpointSlugs(session.page, MOBILITY);
  const answer = await session.page.request.post(`/api/v1/projects/${MOBILITY}/duplicate`, {
    headers: { "x-csrf-token": await csrf(session.context) },
    data: { name: COPY, parameters: { [PARAMETER]: COPY_VALUE } },
  });
  const change = (await json<{ metadata: { name: string } }>(answer, `duplicate ${MOBILITY}`)).metadata.name;
  await approveTyped(session.page, COPY, change);
  await expect.poll(async () => (await session.page.request.get(`/api/v1/projects/${COPY}/endpoints`)).ok(), { timeout: 300_000 }).toBe(true);

  const copy = await endpointSlugs(session.page, COPY);
  measured(info, "origin Endpoint slugs", before);
  measured(info, "copy Endpoint slugs", copy);
  expect(Object.keys(copy).sort(), "the copy holds the origin's Endpoints").toEqual(Object.keys(before).sort());
  for (const slug of Object.values(copy)) {
    expect(Object.values(before), `the copy answers at ${slug}, never at an origin's slug`).not.toContain(slug);
  }
  expect(await endpointSlugs(session.page, MOBILITY), "the origin's Endpoints are unchanged").toEqual(before);

  const copySlug = copy[Object.keys(before).find((name) => before[name] === endpointSlug) ?? ""] ?? "";
  await expect.poll(() => servedDescription(session.page, copySlug), { timeout: 600_000, intervals: [5_000] }).toContain(COPY_VALUE);
  expect(await servedDescription(session.page, endpointSlug), "the origin keeps its own value").toContain(VALUE);
  measured(info, "served values", { origin: await servedDescription(session.page, endpointSlug), copy: await servedDescription(session.page, copySlug) });
});

test("5. helsinki exported as git imports under a new slug with new parameters, heads equal, applications built (MF-45, MF-46)", async ({ browser }, info) => {
  const session = await stewardSession(browser);
  const person = forgeSteward;
  if (!person) throw new Error("step 2 made no forge session");
  const exported = await session.page.request.get(`/api/v1/projects/${HELSINKI}/export?format=git`);
  expect(exported.status(), `export ${HELSINKI} as git`).toBe(200);
  const archive = await exported.body();
  measured(info, "archive", `${archive.length} bytes`);

  const send = async (parameters: Record<string, unknown>, dryRun: boolean) =>
    session.page.request.post(`/api/v1/projects/${IMPORTED}/import?format=git${dryRun ? "&dryRun=All" : ""}`, {
      headers: { "x-csrf-token": await csrf(session.context) },
      multipart: {
        file: { name: `${IMPORTED}-git.zip`, mimeType: "application/zip", buffer: archive },
        parameters: JSON.stringify(parameters),
      },
    });
  const plan = await json<{ repositories: { name: string; role: string; repository: string; head: string }[]; parameters?: Record<string, { type: string; default?: unknown; enum?: unknown[] }> }>(
    await send({}, true),
    "the dry run of the import",
  );
  const applications = plan.repositories.filter((repository) => repository.role === "application");
  measured(info, "planned repositories", plan.repositories.map((r) => `${r.role} ${r.repository} @ ${r.head.slice(0, 12)}`));
  expect(applications, `${HELSINKI} travels with its three applications`).toHaveLength(3);

  // The parameter form, as the import dialog fills it: every declared knob gets a value of its own.
  const parameters: Record<string, unknown> = {};
  for (const [name, declaration] of Object.entries(plan.parameters ?? {})) {
    if (declaration.enum?.length) parameters[name] = declaration.enum[declaration.enum.length - 1];
    else if (declaration.type === "integer" || declaration.type === "number") parameters[name] = 7;
    else if (declaration.type === "boolean") parameters[name] = !(declaration.default ?? false);
    else if (declaration.type === "secret") parameters[name] = `t2647-${name}`;
    else parameters[name] = `T-2647 ${name} ${SUFFIX}`;
  }
  measured(info, "parameter values given", parameters);
  const change = (await json<{ metadata: { name: string } }>(await send(parameters, false), `import ${IMPORTED}`)).metadata.name;
  await approveTyped(session.page, IMPORTED, change);
  await expect.poll(async () => (await session.page.request.get(`/api/v1/projects/${IMPORTED}`)).ok(), { timeout: 300_000 }).toBe(true);
  expect((await registry(session.page, IMPORTED)).parameters, "the values landed in the registry entry").toEqual(parameters);

  for (const repository of plan.repositories) {
    const branch = await json<{ commit: { id: string } }>(
      await forgeApi(person, "GET", `/repos/${FORGE_ORG}/${repository.repository}/branches/main`),
      `main of ${repository.repository}`,
    );
    measured(info, `head of ${repository.repository}`, branch.commit.id);
    expect(branch.commit.id, `${repository.repository} ends where the export did`).toBe(repository.head);
  }

  const apps = await json<{ items: { metadata: { name: string } }[] }>(await session.page.request.get(`/api/v1/projects/${IMPORTED}/apps`), `the apps of ${IMPORTED}`);
  expect(apps.items, "the imported project holds the three applications").toHaveLength(3);
  for (const app of apps.items) {
    const started = Date.now();
    await session.page.goto(`/projects/${IMPORTED}/apps/${app.metadata.name}/open?lang=en`, { waitUntil: "load" });
    await expect(session.page.frameLocator("iframe[sandbox]").locator("body *").first()).toBeVisible({ timeout: 900_000 });
    measured(info, `${app.metadata.name} built and open`, `${Math.round((Date.now() - started) / 1000)} s`);
  }
});

test("6. an app-builder run of the imported project reaches its own repositories only (AG-86)", async ({ browser }, info) => {
  test.skip(process.env.JC_LIVE_MODEL_RUN !== "1", "a live model run is paid: JC_LIVE_MODEL_RUN=1 with the owner's go (6 USD guard, owner 2026-10-07)");
  const session = await stewardSession(browser);
  const person = forgeSteward;
  if (!person) throw new Error("step 2 made no forge session");
  const endpoints = await json<{ items: { metadata: { name: string }; spec: { contextSpaceRef?: { name?: string } } }[] }>(
    await session.page.request.get(`/api/v1/projects/${IMPORTED}/endpoints`),
    `the Endpoints of ${IMPORTED}`,
  );
  const endpoint = endpoints.items[0];
  expect(endpoint, `${IMPORTED} holds an Endpoint to build on`).toBeDefined();
  const since = new Date().toISOString();
  const run = await json<{ id: string }>(
    await session.page.request.post(`/api/v1/projects/${IMPORTED}/agent-runs`, {
      headers: { "x-csrf-token": await csrf(session.context), "x-jc-run-origin": "journey" },
      data: {
        appName: `t2647-${SUFFIX}`,
        endpointName: endpoint.metadata.name,
        appClass: "ui-rust",
        visibility: "project",
        prompt: "A one-page list of the entities this Endpoint serves, with their names.",
        dataNeeds: [{ contextSpaceRef: { kind: "ContextSpace", name: endpoint.spec.contextSpaceRef?.name ?? "" }, operations: ["queryEntity"] }],
      },
    }),
    "start the app-builder run",
  );
  const state = async () =>
    (await json<{ status?: { phase?: string } }>(await session.page.request.get(`/api/v1/projects/${IMPORTED}/agent-runs/${run.id}`), "the run")).status?.phase ?? "";
  await expect.poll(state, { timeout: 1_500_000, intervals: [15_000] }).toMatch(/^(AwaitingApproval|Succeeded|Failed|Cancelled)$/);
  measured(info, "run", `${run.id} ended ${await state()}`);
  await session.page.request.post(`/api/v1/projects/${IMPORTED}/agent-runs/${run.id}/cancel`, {
    headers: { "x-csrf-token": await csrf(session.context) },
  });

  const own = new Set(
    (await json<{ name: string }[]>(await forgeApi(person, "GET", `/orgs/${FORGE_ORG}/repos?limit=200`), "the organization's repositories"))
      .map((repository) => repository.name),
  );
  const reached = new Set<string>();
  for (const name of own) {
    const feeds = await json<{ created: string }[]>(await forgeApi(person, "GET", `/repos/${FORGE_ORG}/${name}/activities/feeds?limit=50`), `activity of ${name}`);
    if (feeds.some((feed) => feed.created >= since)) reached.add(name);
  }
  measured(info, "repositories with forge activity during the run", [...reached]);
  for (const name of reached) {
    expect(name === IMPORTED || name.startsWith(`${IMPORTED}_`), `${name} is ${IMPORTED}'s own repository`).toBe(true);
  }
});
