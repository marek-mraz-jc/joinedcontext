/**
 * Ready for the demo, the project's life (T-2746, TS-26): demo.steward opens a project from the
 * sidebar, duplicates it, and deletes what it made, each a red change the administrator approves
 * with the name typed back (PF-58). In between the steward opens a branch of the new project and
 * renews it (TEN-12, CHG-15, T-7213): the branch is a project of its own named `b-{id}`, so its
 * name never reaches it on the parent's address, and its row stays as a tombstone after the
 * parent goes. The journey's own project keeps the per-project branch limit from filling on dev.
 *
 * Nightly, not hourly (`sweep.sh`'s list): a deleted project's name stays reserved for 30 days,
 * and on layout 2 every project and copy is a forge repository. A project in the organization's
 * configuration repository (layout 1, dev's default) cannot be duplicated, and the page says why;
 * on layout 2 the copy is made, approved and deleted too.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, approve, csrf, signIn } from "./portal";

const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const PROJ = `rdy-proj-${SUFFIX}`;
const COPY = `rdy-copy-${SUFFIX}`;

test.setTimeout(1_800_000);

/** Approves the change a notice links to, typing back the name the approval page asks for. */
async function approveLinked(page: Page): Promise<void> {
  const link = page.getByRole("link", { name: "Review it in Approvals" });
  await expect(link).toBeVisible({ timeout: 60_000 });
  const href = (await link.getAttribute("href")) ?? "";
  const [, , project, , change] = href.split(/[/?#]/);
  expect(change, `the notice links to a change: ${href}`).toMatch(/^chg-/);
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const typed = page.locator("#confirm-resource-name");
  await expect(typed).toBeVisible({ timeout: 60_000 });
  await approve(page, project, change, (await typed.getAttribute("placeholder")) ?? "");
}

interface Branch {
  id: string;
  name: string;
  parent: string;
  expiresAt: string;
}

/** Opens a branch of `project` and renews it once, checking what each door answers. */
async function branchAndRenew(page: Page, token: string, project: string): Promise<void> {
  const headers = { "x-csrf-token": token };
  const opened = await page.request.post(`/api/v1/projects/${project}/branches`, { headers });
  expect(opened.status(), await opened.text()).toBe(201);
  const branch = (await opened.json()) as Branch;
  expect(branch.parent).toBe(project);
  expect(branch.name).toBe(`b-${branch.id}`);
  // RTE-05: a branch's name resolves only on its own id address, never on the parent's.
  expect(await exists(page, branch.name)).toBe(404);
  const renewed = await page.request.post(`/api/v1/projects/${project}/branches/${branch.id}/renewal`, { headers });
  expect(renewed.status(), await renewed.text()).toBe(200);
  const later = (await renewed.json()) as Branch;
  expect(Date.parse(later.expiresAt)).toBeGreaterThan(Date.parse(branch.expiresAt));
}

async function exists(page: Page, project: string): Promise<number> {
  return (await page.request.get(`/api/v1/projects/${project}`)).status();
}

/** Deletes a project from its danger tab and waits until it is gone. */
async function deleteProject(page: Page, project: string): Promise<void> {
  await page.goto(`/projects/${project}/settings/danger?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: `Delete project ${project}`, exact: true }).click();
  const dialog = page.getByRole("dialog", { name: `Delete project ${project}` });
  await expect(dialog.getByText(/stays reserved for \d+ days/)).toBeVisible();
  await dialog.getByLabel(`Type ${project} to confirm`).fill(project);
  await dialog.getByRole("button", { name: "Propose deleting the project" }).click();
  await approveLinked(page);
  await expect
    .poll(() => exists(page, project), { timeout: 600_000, intervals: [10_000], message: `${project} is deleted` })
    .toBe(404);
}

test("a project is opened, branched and renewed, duplicated where it can be, and deleted", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/projects/helsinki/spaces?lang=en");
  const page = steward.page;
  let opened = false;
  let copied = false;
  try {
    // New project is the project switcher's last item (T-7404).
    await page.getByRole("button", { name: "Projects" }).click();
    await page.getByRole("menuitem", { name: "New project" }).click();
    const dialog = page.getByRole("dialog", { name: "Open a project" });
    await dialog.getByLabel(/^Name/).fill(PROJ);
    await dialog.getByLabel("Display name").fill(`Readiness ${SUFFIX}`);
    await dialog.getByRole("button", { name: "Open project" }).click();
    await approveLinked(page);
    opened = true;
    await expect
      .poll(() => exists(page, PROJ), { timeout: 600_000, intervals: [10_000], message: `${PROJ} is opened` })
      .toBe(200);

    await branchAndRenew(page, await csrf(steward.context), PROJ);

    await page.goto(`/projects/${PROJ}/settings/general?lang=en`, { waitUntil: "load" });
    const duplicate = page.getByRole("button", { name: "Duplicate", exact: true });
    await expect(duplicate).toBeVisible({ timeout: 60_000 });
    if ((await duplicate.getAttribute("aria-disabled")) === "true") {
      // Layout 1: the project lives in the organization's repository, which is not copied whole.
      await expect(duplicate).toHaveAttribute("title", "Only a project in a repository of its own can be duplicated.");
    } else {
      await duplicate.click();
      const copy = page.getByRole("dialog", { name: `Duplicate ${PROJ}` });
      await copy.getByLabel(/^Name/).fill(COPY);
      await copy.getByRole("button", { name: "Propose the copy" }).click();
      await approveLinked(page);
      copied = true;
      await expect
        .poll(() => exists(page, COPY), { timeout: 600_000, intervals: [10_000], message: `${COPY} is made` })
        .toBe(200);
    }
  } finally {
    if (copied) {
      await deleteProject(page, COPY);
    }
    if (opened) {
      await deleteProject(page, PROJ);
    }
    await steward.context.close();
  }
});
