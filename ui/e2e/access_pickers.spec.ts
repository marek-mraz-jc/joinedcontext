import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// The access side's references are picked, not typed (T-4947): a person from the realm's people,
// a group or role from the organization's lists, an entity type from the project's models, a
// manifest kind from the person's rights. `vite preview` has no Portal API, so it is answered here.

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, namespace: string, spec: unknown = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace },
  spec,
});
const KINDS = ["ContextSpace", "Endpoint", "Group", "Person", "Pipeline", "Policy", "Role", "RoleBinding", "ServiceAccount"];
// People are named by their Keycloak id in manifests (ADR-N-061).
const JANA = "a1b2c3d4-0000-4000-8000-000000000001";
const PETRA = "a1b2c3d4-0000-4000-8000-000000000002";
const MAREK = "a1b2c3d4-0000-4000-8000-000000000003";
const PEOPLE = [
  { id: JANA, email: "jana.novak@example.org", firstName: "Jana", lastName: "Novák", enabled: true, emailVerified: true, requiredActions: [] },
  { id: PETRA, email: "petra.kral@example.org", firstName: "Petra", lastName: "Kráľ", enabled: true, emailVerified: true, requiredActions: [] },
  { id: MAREK, email: "marek.bily@example.org", firstName: "Marek", lastName: "Biely", enabled: true, emailVerified: true, requiredActions: [] },
];

// A Change on the group that waits for approval, as the changes list names it (T-5605).
const OPEN = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  author: { name: "Petra Kráľ", email: "petra.kral@example.org" },
  createdAt: "2026-10-25T08:00:00Z",
  metadata: { name: "chg-0000001f", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
  summary: { key: "change.summary.update", params: { kind: "Group", name: "stewards", fields: 1 } },
};

async function mock(page: Page, people = PEOPLE, open: unknown[] = []): Promise<unknown[]> {
  const proposed: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (route.request().method() === "PUT") {
      if (url.searchParams.get("dryRun") === "All") return json({ valid: true, verdict: { ok: true } });
      proposed.push(route.request().postDataJSON());
      return json({ ...OPEN, metadata: { name: "chg-0000002a", namespace: "org" } }, 202);
    }
    if (path === "/api/v1/projects/org/changes") return json(LIST(open));
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "admin", name: "Ada Admin", email: "admin@example.org", roles: [] });
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", grants: [{ role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: KINDS, verbs: ["read", "propose", "update", "approve", "delete"] } }] });
    }
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path === "/api/v1/organization/people") {
      const search = (url.searchParams.get("search") ?? "").toLowerCase();
      const found = people.filter((p) => `${p.firstName} ${p.lastName} ${p.email}`.toLowerCase().includes(search));
      return json({ items: found, total: found.length });
    }
    if (path === "/api/v1/organization/people/names") {
      const ids = (url.searchParams.get("ids") ?? "").split(",");
      const items = people.filter((p) => ids.includes(p.id)).map((p) => ({ id: p.id, name: `${p.firstName} ${p.lastName}` }));
      return json({ items });
    }
    if (path === "/api/v1/organization/people/name-by-id") {
      const dry = url.searchParams.get("dryRun") === "true";
      return json({
        repositories: [
          { repository: "organization", manifests: ["Group org/stewards", "RoleBinding org/admins"], ...(dry ? {} : { change: "chg-0000003b" }) },
          { repository: "helsinki", manifests: ["Policy helsinki/air-readers"], ...(dry ? {} : { change: "chg-0000003c" }) },
        ],
        unresolved: ["jan.horak@example.org"],
      });
    }
    if (path === "/api/v1/projects/org/groups") return json(LIST([manifest("Group", "stewards", "org", { members: [{ user: JANA }] }), manifest("Group", "analysts", "org", { members: [] })]));
    if (path === "/api/v1/projects/org/groups/stewards") return json(manifest("Group", "stewards", "org", { members: [{ user: JANA }] }));
    if (path === "/api/v1/projects/org/roles") return json(LIST([manifest("Role", "steward", "org", { rules: [{ kinds: ["Endpoint"], verbs: ["read", "propose"] }] }), manifest("Role", "reader", "org", { rules: [{ kinds: ["Endpoint"], verbs: ["read"] }] })]));
    if (path === "/api/v1/projects/helsinki/endpoints") return json(LIST([manifest("Endpoint", "air", "helsinki", { slug: "air-quality", contextSpaceRef: "air" })]));
    if (path === "/api/v1/organization/datamodels") {
      return json({ ...LIST([{ name: "air-quality", level: "project", project: "helsinki", space: "air", version: "1.0.0", lifecycle: "published", classes: ["AirQualityObserved", "WeatherObserved"] }]), smartDataModels: [] });
    }
    if (path.endsWith("/datamodels/air-quality/source")) return route.fulfill({ status: 200, contentType: "text/yaml", body: "classes: {}\n" });
    return json(LIST([]));
  });
  return proposed;
}

/** What a person's paste of a copied column of e-mails hands the field. */
async function paste(page: Page, label: string, text: string): Promise<void> {
  const field = page.getByRole("combobox", { name: label });
  await field.click();
  await field.evaluate((input, pasted) => {
    const data = new DataTransfer();
    data.setData("text/plain", pasted);
    input.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, text);
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`access pickers at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("grant a role: the person is picked from the realm's people, searched as typed", async ({ page }) => {
      await mock(page);
      await page.goto("/organization/members?lang=en");
      await page.getByRole("button", { name: "Grant a role" }).first().click();
      const person = page.getByRole("combobox", { name: "Username or e-mail" });
      await shot(page, `grant-closed-${width}`);
      await person.click();
      await expect(page.getByRole("option", { name: /Petra Kráľ/ })).toBeVisible();
      await shot(page, `grant-open-${width}`);
      await person.fill("jana");
      await expect(page.getByRole("option", { name: /Jana Novák/ })).toBeVisible();
      await expect(page.getByRole("option", { name: /Petra/ })).toHaveCount(0);
      await shot(page, `grant-searching-${width}`);
      await page.getByRole("option", { name: /Jana Novák/ }).click();
      await expect(page.getByText("jana.novak@example.org")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `grant-chosen-${width}`);
    });

    test("grant a role: someone not in the organization yet is sent to the People page to be invited", async ({ page }) => {
      await mock(page, []);
      await page.goto("/organization/members?lang=en");
      await page.getByRole("button", { name: "Grant a role" }).first().click();
      const person = page.getByRole("combobox", { name: "Username or e-mail" });
      await person.click();
      await expect(page.getByText(/Nobody matches/)).toBeVisible();
      await shot(page, `grant-empty-${width}`);
      await person.fill("eva");
      await expect(page.getByText("Type a name or the person's full e-mail address.")).toBeVisible();
      await person.fill("eva.new@example.org");
      await expect(page.getByText("Nobody with eva.new@example.org is in the organization yet: invite them on the People page first")).toBeVisible();
      await shot(page, `grant-invite-${width}`);
    });

    test("a group's new member is picked, the members already in it are not offered", async ({ page }) => {
      await mock(page);
      await page.goto("/organization/groups/stewards?lang=en");
      const person = page.getByRole("combobox", { name: "Person" });
      await person.click();
      await expect(page.getByRole("option", { name: /Petra Kráľ/ })).toBeVisible();
      await expect(page.getByRole("option", { name: /Jana Novák/ })).toHaveCount(0);
      await shot(page, `group-member-open-${width}`);
    });

    test("a group takes a pasted list of people in one Change, and names what is not an e-mail", async ({ page }) => {
      const proposed = await mock(page);
      await page.goto("/organization/groups/stewards?lang=en");
      await shot(page, `group-members-empty-${width}`);
      await paste(page, "Person", "petra.kral@example.org\nmarek.bily@example.org\njana.novak@example.org\njan.horak@example.org\njana.novak");
      await page.getByRole("option", { name: "Add the e-mails of the list (4)" }).click();
      await expect(page.getByText(/Not added, nobody in the organization has these addresses: .*jan\.horak@example\.org/)).toBeVisible();
      await shot(page, `group-members-pasted-${width}`);
      await page.getByRole("button", { name: "Add members (2)" }).click();
      await expect(page.getByRole("link", { name: "Review it in Approvals" })).toBeVisible();
      expect(proposed).toHaveLength(1);
      expect((proposed[0] as { spec: { members: unknown[] } }).spec.members).toEqual([
        { user: JANA },
        { user: PETRA },
        { user: MAREK },
      ]);
      await shot(page, `group-members-proposed-${width}`);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("a group with a Change waiting holds its member edits and links the change", async ({ page }) => {
      const proposed = await mock(page, PEOPLE, [OPEN]);
      await page.goto("/organization/groups/stewards?lang=en");
      await expect(page.getByRole("link", { name: "Open change chg-0000001f" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Add member" })).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByRole("button", { name: "Remove Jana Novák from the group" })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `group-members-waiting-${width}`);
      expect(proposed).toEqual([]);
    });

    test("a group's page names each member beside their id", async ({ page }) => {
      await mock(page);
      await page.goto("/organization/groups/stewards?lang=en");
      // The name is the link; the id rides in its address (ADR-N-061), no longer printed beside it.
      await expect(page.getByRole("link", { name: "Jana Novák", exact: true }).first()).toHaveAttribute("href", `/organization/people/${JANA}`);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `group-names-${width}`);
    });

    test("people: an administrator checks which manifests still name people by e-mail, then proposes one Change per repository", async ({ page }) => {
      await mock(page);
      await page.goto("/organization/people?lang=en");
      const panel = page.getByRole("region", { name: "Name people by id" });
      await expect(panel.getByRole("button", { name: "Propose the Changes" })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `name-by-id-start-${width}`);
      await panel.getByRole("button", { name: "Check the repositories" }).click();
      await expect(panel.getByText("3 manifest(s) in 2 repository(ies) still name people by e-mail or username:")).toBeVisible();
      await expect(panel.getByText(/jan\.horak@example\.org/)).toBeVisible();
      await panel.scrollIntoViewIfNeeded();
      await shot(page, `name-by-id-checked-${width}`);
      await panel.getByRole("button", { name: "Propose the Changes" }).click();
      await expect(panel.getByText("Proposed 2 Change(s); each waits for approval:")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Change chg-0000003b" })).toHaveAttribute("href", "/projects/org/approvals/chg-0000003b");
      await expect(panel.getByRole("link", { name: "Change chg-0000003c" })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-0000003c");
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `name-by-id-proposed-${width}`);
    });

    test("try a person: the group and the entity type are picked", async ({ page }) => {
      await mock(page);
      await page.goto("/projects/helsinki/policies?lang=en");
      await page.getByLabel(/^Who/).selectOption("group");
      await page.getByRole("combobox", { name: /A member of a group/ }).click();
      await expect(page.getByRole("option", { name: /analysts/ })).toBeVisible();
      await page.getByRole("option", { name: /stewards/ }).click();
      await page.getByRole("combobox", { name: "Entity type" }).click();
      await expect(page.getByRole("option", { name: /AirQualityObserved/ })).toBeVisible();
      await shot(page, `try-type-open-${width}`);
      await page.getByRole("option", { name: /AirQualityObserved/ }).click();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `try-chosen-${width}`);
    });

    test("approval history: the kind is a choice of the kinds the person holds", async ({ page }) => {
      await mock(page);
      await page.goto("/projects/helsinki/approvals?lang=en");
      await page.getByRole("tab", { name: "History" }).click();
      const kind = page.getByLabel("Kind");
      await expect(kind.locator("option", { hasText: "Endpoint" })).toHaveCount(1);
      await kind.selectOption("Endpoint");
      await shot(page, `history-kind-${width}`);
    });
  });
}
