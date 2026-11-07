/**
 * T-2729 — what other projects share with this one, on dev (EP-14, EP-15, UI-44).
 *
 * `/projects/{project}/shared` is the old address of the list; it lands on the Endpoints grid, whose
 * shared rows carry a "Shared by <project>" Source chip (T-0706, T-6169), and a bookmark or the
 * assistant's navigation must keep reaching it. The shared rows are the organization's endpoints that admit this project, read
 * from the source projects: this journey reads the same lists through the API and holds the
 * section to the audience rule itself — `organization` and `public` admit every project,
 * `project-list` the projects it names, anything else none (EP-14). dev holds three projects
 * since T-2455, so there is something on the other side to share.
 *
 * The viewer reads the same rows and meets "Use in this project" in the row menu refused with its
 * reason: referencing a space is a proposal, and a viewer proposes nothing.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";

test.setTimeout(240_000);

interface Endpoint {
  metadata: { name: string; namespace?: string; title?: unknown };
  spec: { audience?: string; allowedProjects?: string[] };
}

/** The endpoints of the other projects this person reads that admit `project` (EP-14). */
async function sharedWith(page: Page, project: string): Promise<{ source: string; name: string }[]> {
  const projects = await page.request.get("/api/v1/projects");
  expect(projects.ok()).toBe(true);
  const names = ((await projects.json()) as { items: { name: string }[] }).items.map((item) => item.name);
  const found: { source: string; name: string }[] = [];
  for (const source of names.filter((name) => name !== project)) {
    const answer = await page.request.get(`/api/v1/projects/${source}/endpoints`);
    if (!answer.ok()) continue;
    for (const endpoint of ((await answer.json()) as { items: Endpoint[] }).items) {
      const { audience, allowedProjects = [] } = endpoint.spec;
      const admitted =
        audience === "organization" || audience === "public" || (audience === "project-list" && allowedProjects.includes(project));
      if (admitted) found.push({ source, name: endpoint.metadata.name });
    }
  }
  return found;
}

/** The grid the old address lands on, as a person reads it. */
async function section(page: Page, project: string) {
  await page.goto(`/projects/${project}/shared?lang=en`, { waitUntil: "load" });
  await expect(page, "the old address lands on the Endpoints page").toHaveURL(new RegExp(`/projects/${project}/endpoints$`));
  const table = page.getByRole("table", { name: "Endpoints" });
  const empty = page.getByText("This project publishes no endpoint yet.");
  await expect(table.or(empty)).toBeVisible({ timeout: 60_000 });
  return table;
}

test("the old address lands on what other projects share with this one, and the list follows the audience rule", async ({
  browser,
}) => {
  const project = "banskabystrica";
  const { context, page } = await signIn(browser, STEWARD, `/projects/${project}/endpoints?lang=en`);
  try {
    const expected = await sharedWith(page, project);
    const table = await section(page, project);
    for (const { source, name } of expected) {
      const row = table.getByRole("row").filter({ hasText: `Shared by ${source}` }).filter({ hasText: name });
      await expect(row.first(), `${source}/${name} is listed as shared`).toBeVisible();
    }
    await expect(table.getByText(`Shared by ${project}`), "the project shares nothing with itself").toHaveCount(0);
  } finally {
    await context.close();
  }
});

test("a viewer reads what is shared and is refused referencing it, with the reason", async ({ browser }) => {
  const project = "helsinki";
  const { context, page } = await signIn(browser, VIEWER, `/projects/${project}/endpoints?lang=en`);
  try {
    const table = await section(page, project);
    // A shared row not referenced yet offers Use in this project in its menu, closed with the verb.
    const offers = table.getByRole("row").filter({ hasText: /Shared by / }).filter({ hasNotText: "Referenced" });
    if ((await offers.count()) > 0) {
      await offers.first().getByRole("button", { name: /More actions/ }).click();
      const use = page.getByRole("menuitem", { name: /^Use in this project/ });
      await expect(use).toHaveAttribute("aria-disabled", "true");
      await expect(use).toContainText(/propose/);
      await page.keyboard.press("Escape");
    }
  } finally {
    await context.close();
  }
});
