/**
 * T-2606 — Project settings on dev: a project's own role, bound to a group, reaching a member of
 * the group and nothing further (UI-76, PF-50, PF-52, PF-64, PF-68, PF-69, PF-70, UI-44;
 * Architecture/09 §14.2).
 *
 * `demo.steward` administers the organization (`org-admin`), so they approve their own red-lane
 * changes as the administrator exception (PF-58). The seed's `approver` role carries no verb on
 * `Role`, `RoleBinding` or `Group` (`helsinki-role-approver.yaml`), so the approval of a grant is
 * theirs, not the approver's.
 *
 * The steward writes the group (`demo.viewer` its one member) and helsinki's own role
 * `t2606-pipeline-author` (propose on Pipeline), then binds the group to the role on the Members
 * tab. `demo.viewer`, signed in afresh so their token carries the group, holds the role in
 * helsinki: New pipeline is theirs, New endpoint stays disabled with the reason.
 *
 * Nothing is left on dev: binding, role and group are removed again, each a change approved.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, VIEWER, approve, checkManifest, csrf, proposedChange, removeCompletely, signIn } from "./portal";

const PROJECT = "helsinki";
const ROLE = "t2606-pipeline-author";
const GROUP = "t2606-authors";

test.setTimeout(900_000);

type Session = { context: BrowserContext; page: Page };

/** Approves a red-lane change, typing back the name the approval page asks for (CC-19). */
async function approveTyped(page: Page, project: string, change: string): Promise<void> {
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const input = page.locator("#confirm-resource-name");
  await expect(input).toBeVisible({ timeout: 60_000 });
  await approve(page, project, change, (await input.getAttribute("placeholder")) ?? "");
}

/** Proposes one manifest through the resource route and approves it, as the steward. */
async function proposeAndApprove(owner: Session, project: string, plural: string, manifest: { kind: string } & Record<string, unknown>): Promise<void> {
  await checkManifest(owner.page, owner.context, project, manifest);
  const answer = await owner.page.request.post(`/api/v1/projects/${project}/${plural}`, {
    headers: { "x-csrf-token": await csrf(owner.context) },
    data: manifest,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  await approveTyped(owner.page, project, change);
}

/** The names of the organization's bindings that name the journey's group. */
async function bindingsOfGroup(page: Page): Promise<string[]> {
  const answer = await page.request.get("/api/v1/projects/org/rolebindings");
  expect(answer.ok()).toBe(true);
  const items = ((await answer.json()) as {
    items: { metadata: { name: string }; spec: { subjects?: { group?: string }[] } }[];
  }).items;
  return items
    .filter((binding) => (binding.spec.subjects ?? []).some((subject) => subject.group === GROUP))
    .map((binding) => binding.metadata.name);
}

async function exists(page: Page, route: string): Promise<boolean> {
  return (await page.request.get(route)).status() === 200;
}

test("a project role bound to a group reaches the group's member, and only as far as it says", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/settings/general?lang=en`);
  let viewer: Session | null = null;
  try {
    const { page } = steward;
    expect(await bindingsOfGroup(page), "nothing is left over from an earlier run").toEqual([]);

    // The group and the role, as manifests: the group's sync to Keycloak is what a fresh token
    // of its member carries.
    await proposeAndApprove(steward, "org", "groups", {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Group",
      metadata: { name: GROUP, namespace: "org" },
      spec: { description: "Authors of the T-2606 live journey", members: [{ user: VIEWER.user }] },
    });
    await proposeAndApprove(steward, PROJECT, "roles", {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Role",
      metadata: { name: ROLE, namespace: PROJECT },
      spec: { rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }] },
    });
    await expect
      .poll(
        async () => {
          const answer = await page.request.get(`/api/v1/projects/org/groups/${GROUP}`);
          const conditions =
            ((await answer.json()) as { status?: { conditions?: { type: string; status: string }[] } }).status
              ?.conditions ?? [];
          return conditions.find((condition) => condition.type === "GroupSynced")?.status ?? "";
        },
        { timeout: 300_000, message: "the group reaches Keycloak" },
      )
      .toBe("True");

    // Roles: helsinki's own role is listed where the steward writes project roles.
    await page.goto(`/projects/${PROJECT}/settings/roles?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("row", { name: new RegExp(ROLE) })).toBeVisible({ timeout: 120_000 });

    // Members: the grant, as a person makes it.
    await page.getByRole("tab", { name: "Members" }).click();
    await expect(page).toHaveURL(/\/settings\/members/);
    await page.getByRole("button", { name: "Grant a role" }).click();
    const form = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
    await form.getByLabel("Give it to").selectOption("group");
    await form.getByLabel("Group name").fill(GROUP);
    await form.getByLabel("Role", { exact: true }).selectOption(ROLE);
    // A project role is bound in its project; the organization is not offered (PF-69).
    await expect(form.getByLabel("Where it applies").locator("option", { hasText: "The whole organization" })).toHaveCount(0);
    await form.getByRole("button", { name: "Propose grant" }).click();
    const change = await proposedChange(page);
    await approveTyped(page, PROJECT, change);
    await expect.poll(() => bindingsOfGroup(page), { timeout: 180_000 }).toHaveLength(1);

    // The member of the group, signed in afresh.
    viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/pipelines?lang=en`);
    const member = viewer;
    await expect
      .poll(
        async () => {
          const answer = await member.page.request.get(`/api/v1/projects/${PROJECT}/permissions/me`);
          const grants = ((await answer.json()) as { grants?: { role?: string }[] }).grants ?? [];
          return grants.map((grant) => grant.role);
        },
        { timeout: 180_000 },
      )
      .toContain(ROLE);
    await member.page.reload({ waitUntil: "load" });
    const create = member.page.getByRole("button", { name: "New pipeline" }).first();
    await expect(create).toBeVisible({ timeout: 60_000 });
    await expect(create).not.toHaveAttribute("aria-disabled", "true");

    await member.page.goto(`/projects/${PROJECT}/endpoints?lang=en`, { waitUntil: "load" });
    const endpoint = member.page.getByRole("button", { name: "New endpoint" }).first();
    await expect(endpoint).toHaveAttribute("aria-disabled", "true", { timeout: 60_000 });
    const reason = await endpoint.getAttribute("aria-describedby");
    expect(reason, "the refusal carries its reason").toBeTruthy();
    await expect(member.page.locator(`[id="${reason}"]`).first()).toContainText(/Endpoint/);
  } finally {
    for (const name of await bindingsOfGroup(steward.page)) {
      await removeCompletely(steward, "org", "rolebindings", name);
    }
    if (await exists(steward.page, `/api/v1/projects/${PROJECT}/roles/${ROLE}`)) {
      await removeCompletely(steward, PROJECT, "roles", ROLE);
    }
    if (await exists(steward.page, `/api/v1/projects/org/groups/${GROUP}`)) {
      await removeCompletely(steward, "org", "groups", GROUP);
    }
    await viewer?.context.close();
    await steward.context.close();
  }
});

test("an old Access link lands on Members, and a viewer meets Delete project refused", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, `/projects/${PROJECT}/access?lang=en`);
  try {
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/settings/members\\?lang=en`));
    await expect(page.getByRole("heading", { level: 1, name: "Project settings" })).toBeVisible();
    await page.getByRole("tab", { name: "Delete or hand over" }).click();
    await expect(page.getByRole("button", { name: `Delete project ${PROJECT}` })).toHaveAttribute(
      "aria-disabled",
      "true",
      { timeout: 60_000 },
    );
    // A tab's routed form: the service account the assistant opens at …/new.
    await page.goto(`/projects/${PROJECT}/settings/service-accounts/new?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { name: "New service account" })).toBeVisible({ timeout: 60_000 });
  } finally {
    await context.close();
  }
});

// T-3277: every project setting is findable by what it is called or what it changes.
test("a steward finds a quota and the hand-over by searching the settings", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/settings/general?lang=en`);
  try {
    const search = page.getByRole("searchbox", { name: "Find a setting" });
    await search.fill("public endpoints");
    const found = page.getByRole("list", { name: "Settings found" });
    await expect(found.getByRole("link").first()).toBeVisible({ timeout: 30_000 });
    await search.fill("hand over");
    await found.getByRole("link", { name: "Hand the project over" }).click();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/settings/danger`));
    await expect(page.getByRole("heading", { name: "Hand the project over" })).toBeVisible();
  } finally {
    await context.close();
  }
});
