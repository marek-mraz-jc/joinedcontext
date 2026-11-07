/**
 * A project's Git export, checked as a new project on dev (T-2729; MF-45, MF-46, CC-88, TS-26).
 *
 * The Portal has no screen for a Git import: `jcctl` and `jc_project_import` are its doors, and
 * both send `POST …/import?format=git`. The journey sends the same request from a signed-in
 * steward's session: the archive of a project that has a repository of its own (layout 2) is
 * exported and checked as a project that does not exist yet, which answers the plan (its
 * repositories and declared parameters) and creates nothing. A viewer, who may not open a
 * project, is refused before the archive is read (PF-65).
 *
 * Only the dry run: a real import creates repositories on the forge and proposes a project to
 * the organization, which the next sweep would have to take apart again.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, csrf, signIn } from "./portal";

const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const NEW_PROJECT = `git-${SUFFIX}`;

test.setTimeout(300_000);

/** The first project dev holds that has a repository of its own: only those export as Git. */
async function layoutTwoProject(page: Page): Promise<string> {
  const listed = await page.request.get("/api/v1/projects");
  expect(listed.ok(), `list projects: ${listed.status()}`).toBe(true);
  const { items } = (await listed.json()) as { items: { name: string }[] };
  for (const { name } of items) {
    const detail = await page.request.get(`/api/v1/projects/${name}`);
    if (!detail.ok()) continue;
    const project = (await detail.json()) as { spec?: { repository?: unknown } };
    if (project.spec?.repository) return name;
  }
  throw new Error(`no project on dev has a repository of its own, so none exports as Git: ${items.map((p) => p.name).join(", ")}`);
}

async function exportGit(page: Page, project: string): Promise<Buffer> {
  const answer = await page.request.get(`/api/v1/projects/${project}/export?format=git`);
  expect(answer.status(), `export ${project} as git: ${await answer.text().catch(() => "")}`).toBe(200);
  const archive = await answer.body();
  expect(archive.length, "an empty archive").toBeGreaterThan(0);
  return archive;
}

async function checkImport(page: Page, archive: Buffer) {
  return page.request.post(`/api/v1/projects/${NEW_PROJECT}/import?format=git&dryRun=All`, {
    headers: { "x-csrf-token": await csrf(page.context()) },
    multipart: { file: { name: `${NEW_PROJECT}-git.zip`, mimeType: "application/zip", buffer: archive } },
  });
}

test("a steward checks a project's Git export as a new project, and nothing is created", async ({ browser }) => {
  const { page } = await signIn(browser, STEWARD, "/projects?lang=en");
  const source = await layoutTwoProject(page);
  const archive = await exportGit(page, source);

  const answer = await checkImport(page, archive);
  expect(answer.status(), await answer.text()).toBe(200);
  const plan = (await answer.json()) as { repositories?: { repository: string }[]; parameters?: Record<string, unknown> };
  const repositories = (plan.repositories ?? []).map((entry) => entry.repository);
  // The project lands under its new slug: its own repository first, then its apps' (MF-45).
  expect(repositories[0], JSON.stringify(plan)).toBe(NEW_PROJECT);
  expect(plan.parameters, "the plan names the parameters the project declares (CC-88)").toBeDefined();

  const created = await page.request.get(`/api/v1/projects/${NEW_PROJECT}`);
  expect(created.status(), "a dry run created the project").toBe(404);
});

test("a viewer is refused a Git import before the archive is read", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/projects?lang=en");
  const archive = await exportGit(steward.page, await layoutTwoProject(steward.page));
  await steward.context.close();

  const { page } = await signIn(browser, VIEWER, "/projects?lang=en");
  const answer = await checkImport(page, archive);
  expect(answer.status(), await answer.text()).toBe(403);
  const created = await page.request.get(`/api/v1/projects/${NEW_PROJECT}`);
  expect(created.status()).toBe(404);
});
