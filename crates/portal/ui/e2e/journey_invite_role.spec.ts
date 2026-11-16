import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { greenVerdict, isCheck } from "./verdict";
import { manifestReadsAsRest } from "./manifestReads";
import { odataBody } from "../tests/odata";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7071, journey 1: an Organization administrator invites a person with a role on one project,
// the person signs in and reads that project only; the administrator removes the role and the
// person loses it. `vite preview` has no Portal API, so the API answers in the browser as the
// server answers each person: a project outside their reach is 404 (PF-59). The approval of each
// role Change is simulated in the stub (`world.bound`): the approver is another person.

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const PETRA_ID = "5f0c2a9e-4b1d-4c3e-9a7f-2d8e6b1c0a43";
const PETRA = {
  id: PETRA_ID,
  email: "petra.kral@example.org",
  firstName: "Petra",
  lastName: "Kráľ",
  locale: "sk",
  enabled: true,
  emailVerified: false,
  requiredActions: ["VERIFY_EMAIL", "UPDATE_PASSWORD"],
  createdAt: "2026-11-20T08:00:00Z",
  lastSeen: null,
  pendingDeletion: null,
  invitationExpires: null,
};
const ADMIN = { subject: "a1", username: "ida", name: "Ida Admin", email: "ida@hel.fi", roles: [] };
const PERSON = { subject: PETRA_ID, username: "petra", name: "Petra Kráľ", email: PETRA.email, roles: [] };
const KINDS = ["Person", "RoleBinding", "Role", "Group", "Organization", "Project", "ContextSpace", "Endpoint"];
const ADMIN_GRANTS = [
  {
    role: "org-admin",
    binding: "admins",
    scope: "organization",
    rule: { kinds: KINDS, verbs: ["read", "create", "update", "disable", "delete", "propose", "approve"] },
  },
];
const VIEWER_GRANTS = [
  { role: "viewer", binding: "petra-viewer-helsinki", scope: "project:helsinki", rule: { kinds: ["ContextSpace", "Endpoint"], verbs: ["read"] } },
];
const BINDING = {
  apiVersion: API,
  kind: "RoleBinding",
  metadata: { name: "petra-viewer-helsinki", namespace: "org" },
  spec: { subjects: [{ user: PETRA_ID }], role: "viewer", scope: { project: "helsinki" } },
};
const change = (namespace: string) => ({
  apiVersion: API,
  kind: "Change",
  metadata: { name: "chg-0000071a", namespace },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
});

interface World {
  /** Whether the binding is merged: the simulated approval of the proposed Change. */
  bound: boolean;
  writes: string[];
}

async function stub(page: Page, persona: "admin" | "petra", world: World): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const reads = persona === "admin" ? ["helsinki", "espoo"] : world.bound ? ["helsinki"] : [];
    if (path.endsWith("/auth/me")) return json(persona === "admin" ? ADMIN : PERSON);
    if (path === "/api/v1/projects") return json(list(reads.map((name) => ({ name }))));
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path === "/api/v1/organization/people") {
      if (method === "POST") {
        world.writes.push(`POST ${path}`);
        return json({ person: PETRA, emailSent: true }, 201);
      }
      return json({ items: world.bound ? [PETRA] : [] });
    }
    if (path.includes("JoinedContext.Portal.portal_person_names")) {
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ items: [{ id: PETRA_ID, name: "Petra Kráľ" }] }) });
    }
    const project = /^\/api\/v1\/projects\/([^/]+)/.exec(path)?.[1];
    if (!project) return json(list([]));
    if (persona === "petra" && project !== "org" && !reads.includes(project)) {
      return route.fulfill({
        status: 404,
        contentType: "application/problem+json",
        body: JSON.stringify({
          type: "https://joinedcontext.com/errors/resource-not-found",
          title: "Resource Not Found",
          status: 404,
          detail: `project '${project}' not found`,
        }),
      });
    }
    if (isCheck(method, request.url())) return json(greenVerdict(request.postData()));
    if (method !== "GET") {
      world.writes.push(`${method} ${path}`);
      if (path.includes("/rolebindings")) world.bound = method !== "DELETE";
      return json(change(project), 202);
    }
    if (path.endsWith("/permissions/me")) {
      return json({ project, bootstrap: false, grants: persona === "admin" ? ADMIN_GRANTS : VIEWER_GRANTS });
    }
    if (path === "/api/v1/projects/org/rolebindings") return json(list(world.bound ? [BINDING] : []));
    if (path === "/api/v1/projects/org/rolebindings/petra-viewer-helsinki") return json(BINDING);
    return json(list([]));
  });
}

async function as(page: Page, persona: "admin" | "petra", world: World): Promise<void> {
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await manifestReadsAsRest(page);
  await stub(page, persona, world);
}

/** Shots for the task's evidence, where `JC_EVIDENCE_DIR` asks for them; no secret is on screen. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  // From the top: a full-page shot of a scrolled page draws the fixed top bar mid-page.
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test(`invite, grant one project, sign in, remove the role, lose it at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const world: World = { bound: false, writes: [] };

    // The administrator invites Petra into helsinki as a Viewer.
    await as(page, "admin", world);
    await page.goto("/organization/people?lang=en");
    await page.getByRole("button", { name: "New person" }).click();
    await page.getByRole("textbox", { name: /^E-mail/ }).fill(PETRA.email);
    await page.getByRole("textbox", { name: /^First name/ }).fill("Petra");
    await page.getByRole("textbox", { name: /^Last name/ }).fill("Kráľ");
    await page.getByRole("combobox", { name: /^Project/ }).selectOption("helsinki");
    await shot(page, `journey-invite-form-${width}`);
    await page.getByRole("button", { name: "Create person" }).click();
    await expect(page.getByText(`The role Viewer for ${PETRA.email} in helsinki is proposed.`)).toBeVisible();
    expect(world.writes).toContain("POST /api/v1/organization/people");
    expect(world.writes.some((write) => write.startsWith("POST /api/v1/projects/org/rolebindings"))).toBe(true);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `journey-invite-proposed-${width}`);

    // Petra signs in: helsinki opens, another project answers as if it did not exist.
    await as(page, "petra", world);
    await page.goto("/projects/helsinki/spaces?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Context Spaces" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `journey-person-project-${width}`);
    await page.goto("/projects/espoo/spaces?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "This page does not exist" })).toBeVisible();
    await expect(page.getByText("This project is not open to you")).toBeVisible();
    await shot(page, `journey-person-refused-${width}`);

    // The administrator removes the role on the project's Members.
    await as(page, "admin", world);
    await page.goto("/projects/helsinki/settings/members?lang=en");
    await expect(page.getByText("Petra Kráľ").first()).toBeVisible();
    await shot(page, `journey-members-filled-${width}`);
    await page.getByRole("button", { name: /Remove/ }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("textbox").fill("petra-viewer-helsinki");
    await dialog.getByRole("button", { name: "Propose removal" }).click();
    await expect.poll(() => world.writes).toContain("DELETE /api/v1/projects/org/rolebindings/petra-viewer-helsinki");
    await shot(page, `journey-members-removal-${width}`);

    // Once the removal is approved, Petra no longer reads helsinki.
    await as(page, "petra", world);
    await page.goto("/projects/helsinki/spaces?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "This page does not exist" })).toBeVisible();
    await shot(page, `journey-person-lost-${width}`);
  });
}
