import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7404 (owner screenshot "Edit Unknown person: org-admin" over a YAML editor): Administration →
// Members → Edit is a form with the person's name, the role as a dropdown and the scope in words;
// the server's refusals (T-7403) stand in it. `vite preview` has no Portal behind it, so the API
// is answered in the browser; the pages are the real build.

const OWNER = "3f1c2b9a-7d4e-4c1a-9b2f-5e6d7c8b9a01";
const IDENTITY = { subject: OWNER, username: "marek", name: "Marek Mráz", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const BINDING = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "RoleBinding",
  metadata: { name: "marek-mraz-admin", namespace: "org" },
  spec: { subjects: [{ user: OWNER }], role: "org-admin", scope: { organization: "marek-mraz" } },
};
const ROLES = ["org-admin", "steward", "viewer"].map((name) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Role",
  metadata: { name, namespace: "org" },
  spec: { rules: [] },
}));
const ADMIN = [{ rule: { kinds: ["RoleBinding", "Role", "Group", "Organization"], verbs: ["read", "propose", "approve", "delete"] }, organization: "marek-mraz" }];
const LAST =
  "this change would leave the Organization marek-mraz without an administrator: after it, no binding at organization scope grants approve and delete on RoleBinding (today: marek-mraz-admin). Bind another person to such a role first, then make this change (PF-03)";
const ABOVE = "a binding may not grant more than its proposer holds: missing approve on Project (PF-52)";

async function stubApi(page: Page, refusal?: { status: number; detail: string }): Promise<string[]> {
  const sent: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200, contentType = "application/json") =>
      route.fulfill({ status, contentType, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(list([{ name: "pilot" }]));
    if (path.endsWith("/permissions/me")) return json({ grants: ADMIN, projects: { creation: { allowed: true } } });
    if (path.includes("portal_person_names")) return json({ value: { items: [{ id: OWNER, name: "Marek Mráz" }] } });
    if (path === "/api/v1/projects/org/rolebindings") return json(list([BINDING]));
    if (path === "/api/v1/projects/org/rolebindings/marek-mraz-admin" && request.method() === "GET") return json(BINDING);
    if (path === "/api/v1/projects/org/roles") return json(list(ROLES));
    if (request.method() === "PUT") {
      sent.push(request.postData() ?? "");
      if (refusal) {
        return json({ type: "https://joinedcontext.com/errors/refused", title: "Refused", status: refusal.status, detail: refusal.detail }, refusal.status, "application/problem+json");
      }
    }
    return json(list([]));
  });
  return sent;
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${process.env.JC_SHOT_PREFIX ?? "after"}-${name}.png`), fullPage: true });
}

test.describe("a member's edit form", () => {
  for (const width of [1440, 400]) {
    test(`names the person and offers the roles at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page);
      await page.goto("/organization/members/marek-mraz-admin/edit?lang=en");
      const role = page.getByRole("combobox", { name: /Role/ });
      await expect(role).toBeVisible();
      await expect(page.locator("dd", { hasText: "Marek Mráz" })).toBeVisible();
      await expect(page.getByText("Unknown person")).toHaveCount(0);
      await expect(page.getByText(OWNER)).toHaveCount(0);
      await expect(page.getByText("apiVersion")).toHaveCount(0);
      await expect(role.locator("option")).toContainText(["org-admin", "steward", "viewer"]);
      await shot(page, `member-form-${width}`);
    });

    for (const [name, refusal, words] of [
      ["refused", { status: 403, detail: ABOVE }, "may not grant more"],
      ["last-admin", { status: 409, detail: LAST }, "without an administrator"],
    ] as const) {
      test(`shows the server's ${name} refusal at ${width} px`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        const sent = await stubApi(page, refusal);
        await page.goto("/organization/members/marek-mraz-admin/edit?lang=en");
        const role = page.getByRole("combobox", { name: /Role/ });
        await role.selectOption("viewer");
        await page.getByRole("button", { name: "Propose change" }).click();
        await expect(page.getByText(words).first()).toBeVisible();
        expect(JSON.parse(sent[0] ?? "{}")).toMatchObject({ spec: { role: "viewer", subjects: [{ user: OWNER }], scope: { organization: "marek-mraz" } } });
        await shot(page, `member-${name}-${width}`);
      });
    }
  }
});
