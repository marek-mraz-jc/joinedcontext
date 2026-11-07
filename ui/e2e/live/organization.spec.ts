/**
 * T-2605 — the Organization page on dev, by the people it is for (UI-75, PF-56, PF-59, PF-52,
 * UI-44; Architecture/09 §14.1).
 *
 * `demo.steward` administers the organization (`org-admin`, `helsinki-rolebinding-admins.yaml`):
 * they open the page from the navigation, read the seeded taxonomy on Roles and the seed's
 * bindings on Members — the `viewers` binding that names the demo people one by one (T-6415,
 * ADR-N-067: no group every signed-in person carries is bound), the approver — and grant `demo.viewer` the `model-editor` role at organization scope. The grant is
 * a red-lane change the administrator approves as the administrator exception (PF-58), and the
 * proof it did something is the viewer's own `permissions/me`, which gains the role.
 *
 * `demo.viewer` meets Members as the sentence that says who can see it, never the list.
 *
 * Nothing is left on dev: the binding is removed again, and it carries an end date of tomorrow,
 * so a run that dies before its clean-up leaves a grant that lapses by itself.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, VIEWER, approve, proposedChange, removeCompletely, signIn } from "./portal";

const ROLE = "model-editor";

test.setTimeout(600_000);

interface Binding {
  metadata: { name: string };
  spec: { role?: string; subjects?: { user?: string }[]; scope?: { organization?: string } };
}

/** The viewer's organization-scope bindings to `ROLE`, as the API lists them. */
async function grantsOfViewer(page: Page): Promise<string[]> {
  const answer = await page.request.get("/api/v1/projects/org/rolebindings");
  expect(answer.ok(), "the administrator reads the organization's bindings").toBe(true);
  const items = ((await answer.json()) as { items: Binding[] }).items;
  return items
    .filter(
      (binding) =>
        binding.spec.role === ROLE &&
        Boolean(binding.spec.scope?.organization) &&
        (binding.spec.subjects ?? []).some((subject) => subject.user === VIEWER.user),
    )
    .map((binding) => binding.metadata.name);
}

/** The roles the viewer holds in `helsinki`, read by the viewer themselves. */
async function viewerRoles(viewer: { page: Page }): Promise<string[]> {
  const answer = await viewer.page.request.get("/api/v1/projects/helsinki/permissions/me");
  expect(answer.ok(), "the viewer reads their own permissions").toBe(true);
  const grants = ((await answer.json()) as { grants?: { role?: string }[] }).grants ?? [];
  return grants.map((grant) => grant.role ?? "");
}

/** Approves a red-lane change, typing back the name the approval page asks for (CC-19). */
async function approveTyped(page: Page, change: string): Promise<void> {
  await page.goto(`/projects/org/approvals/${change}?lang=en`, { waitUntil: "load" });
  const input = page.locator("#confirm-resource-name");
  await expect(input).toBeVisible({ timeout: 60_000 });
  const name = (await input.getAttribute("placeholder")) ?? "";
  expect(name, "the approval page names the resource to type back").not.toBe("");
  await approve(page, "org", change, name);
}

test("an administrator reads the organization and grants a role that reaches the person", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/projects/helsinki/spaces?lang=en");
  let viewer: { context: BrowserContext; page: Page } | null = null;
  try {
    const { page } = steward;
    expect(await grantsOfViewer(page), "no grant is left over from an earlier run").toEqual([]);

    // Reached from the one Administration entry of the top bar (UI-75, T-2879).
    await page.getByRole("banner").getByRole("link", { name: "Administration" }).click();
    await expect(page).toHaveURL(/\/organization\/settings/);
    await expect(page.getByRole("heading", { level: 1, name: "Administration" })).toBeVisible();

    // PF-56: the taxonomy every organization starts from, marked as the seed.
    await page.getByRole("tab", { name: "Roles" }).click();
    await expect(page).toHaveURL(/\/organization\/roles/);
    for (const seeded of ["viewer", "steward", "org-admin"]) {
      await expect(
        page.getByRole("row", { name: new RegExp(`^${seeded}\\b`) }).getByText("seeded"),
        `${seeded} is marked as seeded`,
      ).toBeVisible({ timeout: 60_000 });
    }

    // The seed's organization bindings: the named viewers (T-6415), and the approver's own row
    // (the viewers row names the approver too).
    await page.getByRole("tab", { name: "Members" }).click();
    await expect(page.getByRole("row", { name: /demo\.probe@hel\.fi/ })).toBeVisible({ timeout: 60_000 });
    await expect(
      page.getByRole("row", { name: /demo\.approver@hel\.fi/ }).filter({ hasNotText: "demo.probe@hel.fi" }),
    ).toBeVisible();

    // Groups has its own tab, outside any project.
    await page.getByRole("tab", { name: "Groups" }).click();
    await expect(page.getByRole("heading", { level: 2, name: "Groups" })).toBeVisible();

    // The grant, as a person makes it: who, which role, until tomorrow.
    await page.getByRole("tab", { name: "Members" }).click();
    await page.getByRole("button", { name: "Grant a role" }).click();
    const form = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
    await form.getByLabel("Username or e-mail").fill(VIEWER.user);
    // The label carries the required mark, "Role*", so it is matched from its start.
    await form.getByLabel(/^Role\b/).selectOption(ROLE);
    const tomorrow = new Date(Date.now() + 86_400_000).toISOString().slice(0, 10);
    await form.getByLabel("Until (optional)").fill(tomorrow);
    await form.getByRole("button", { name: "Propose grant" }).click();
    const change = await proposedChange(page);
    await approveTyped(page, change);
    await expect.poll(() => grantsOfViewer(page), { timeout: 180_000 }).toHaveLength(1);

    // The proof it did something: the viewer's own permissions carry the role.
    viewer = await signIn(browser, VIEWER, "/organization/members?lang=en");
    const signedIn = viewer;
    await expect.poll(() => viewerRoles(signedIn), { timeout: 180_000 }).toContain(ROLE);
  } finally {
    for (const name of await grantsOfViewer(steward.page)) {
      await removeCompletely(steward, "org", "rolebindings", name);
    }
    await viewer?.context.close();
    await steward.context.close();
  }
});

test("a viewer is told who can see the members, and the list is never fetched for them", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, "/organization/members?lang=en");
  try {
    const fetched: string[] = [];
    page.on("request", (request) => {
      if (request.url().includes("/rolebindings")) fetched.push(request.url());
    });
    await page.reload({ waitUntil: "load" });
    await expect(
      page.getByText("You cannot see who belongs to this organization; an organization administrator can."),
    ).toBeVisible({ timeout: 60_000 });
    expect(fetched).toEqual([]);

    // UI-44: on Projects, Delete stays in place, refused with the reason.
    await page.getByRole("tab", { name: "Projects" }).click();
    // Exact: "Delete project helsinki-mobility" is the next row's.
    const remove = page.getByRole("button", { name: "Delete project helsinki", exact: true });
    await expect(remove).toHaveAttribute("aria-disabled", "true", { timeout: 60_000 });

    // A tab's routed form: the group the assistant opens at …/new.
    await page.goto("/organization/groups/new?lang=en", { waitUntil: "load" });
    await expect(page.getByRole("heading", { name: "New group" })).toBeVisible({ timeout: 60_000 });
  } finally {
    await context.close();
  }
});

// ADR-N-061 (T-5711): the administrator checks which manifests still name people by e-mail; the
// check proposes nothing, so the walk leaves the repositories as they were.
test("an administrator checks which manifests still name people by e-mail, and nothing is proposed", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/organization/people?lang=en");
  try {
    const { page } = steward;
    const panel = page.getByRole("region", { name: "Name people by id" });
    await expect(panel).toBeVisible({ timeout: 60_000 });
    const asked = page.waitForResponse((response) => response.url().includes("/organization/people/name-by-id?dryRun=true"));
    await panel.getByRole("button", { name: "Check the repositories" }).click();
    expect((await asked).status(), "the check answers the plan").toBe(200);
    await expect(panel.getByRole("status")).toBeVisible();
    await expect(panel.getByText(/Change chg-/)).toHaveCount(0);
  } finally {
    await steward.context.close();
  }
});
