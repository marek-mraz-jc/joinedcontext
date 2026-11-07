/**
 * A group's page (T-2685, PF-95, PF-69, UI-44): its members, its platform roles and its
 * application roles, each edit proposed as the one manifest diff it should be, a project role
 * refused at organization scope with the reason, and every write control disabled with the reason
 * for a caller who may not propose.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/access/GroupPage.tsx and
// src/pages/access/groupRoles.ts.
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { GroupPage } from "../src/pages/access/GroupPage";
import {
  appRolesNaming,
  bindingsNaming,
  bindingWithoutGroup,
  groupBinding,
  refusedPlace,
  withGroupMember,
  withGroupMembers,
} from "../src/pages/access/groupRoles";
import { emailsIn } from "../src/pages/apps/members";
import { expectDenied, expectNoViolations } from "./checks";
import { list, renderPage } from "./page_contract";
import { viewerWriteFindings } from "./pageChecks";

const V = "joinedcontext.com/v1alpha1";
const CHANGE = {
  apiVersion: V,
  kind: "Change",
  metadata: { name: "chg-0000002a", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};

/** The Keycloak id the stub realm gives a person (ADR-N-061): members are ids, never e-mails. */
const PEOPLE = ["jana@hel.fi", "petra@hel.fi", "gone@hel.fi", ...Array.from({ length: 20 }, (_, index) => `person${index + 1}@hel.fi`)];
function idOf(email: string): string {
  return `00000000-0000-4000-8000-${String(PEOPLE.indexOf(email) + 1).padStart(12, "0")}`;
}

const GROUP = {
  apiVersion: V,
  kind: "Group",
  metadata: { name: "stewards", namespace: "org", labels: { team: "city" } },
  spec: { description: "The city's data stewards", members: [{ user: idOf("jana@hel.fi") }] },
};

const binding = (name: string, subjects: unknown[], role: string, scope: unknown) => ({
  apiVersion: V,
  kind: "RoleBinding",
  metadata: { name, namespace: "org" },
  spec: { subjects, role, scope },
});

const BINDINGS = [
  binding("mixed", [{ group: "stewards" }, { user: "jana@hel.fi" }], "steward", { project: "helsinki" }),
  binding("stewards-only", [{ group: "stewards" }], "viewer", { organization: "hel" }),
  binding("other", [{ group: "editors" }], "viewer", { organization: "hel" }),
];

const APP = {
  apiVersion: V,
  kind: "App",
  metadata: { name: "alerts", namespace: "helsinki" },
  spec: {
    kind: "static",
    roles: [{ name: "viewer" }, { name: "steward" }],
    access: [{ role: "viewer", subjects: [{ group: "stewards" }, { user: "petra@hel.fi" }] }],
  },
};

// PF-107 (T-3046): the right on one CKAN instance, given to the group.
const CKAN_ROLE = {
  apiVersion: V,
  kind: "Role",
  metadata: { name: "ckan-admin-helsinki-hel-fi", namespace: "org" },
  spec: {
    rules: [{ kinds: ["CkanInstance"], verbs: ["propose", "approve", "delete"], constraints: [{ field: "metadata.name", in: ["hel-fi"] }] }],
  },
};
const CKAN_BINDING = binding("ckan-admin-helsinki-hel-fi", [{ group: "stewards" }], "ckan-admin-helsinki-hel-fi", { project: "helsinki" });

interface GroupSpec {
  members: { user: string }[];
}

interface Sent {
  method: string;
  path: string;
  dryRun: boolean;
  body: unknown;
}

// A Change on the group that waits for approval (T-5605), as the changes list names it.
const OPEN = {
  apiVersion: V,
  kind: "Change",
  author: { name: "Petra", email: "petra@hel.fi" },
  createdAt: "2026-10-25T08:00:00Z",
  metadata: { name: "chg-0000001f", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
  summary: { key: "change.summary.update", params: { kind: "Group", name: "stewards", fields: 1 } },
};

function renderGroup({ verbs = ["read", "propose", "delete"], people = false, ckan = false, open = [] as unknown[] } = {}) {
  const sent: Sent[] = [];
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
    });
  renderPage(<GroupPage name="stewards" />, {
    path: "/organization/groups/stewards",
    answer: async (url, request) => {
      const path = url.pathname;
      if (path.endsWith("/permissions/me")) {
        return reply(200, {
          project: "org",
          bootstrap: false,
          grants: [
            { rule: { kinds: ["Group", "RoleBinding", "App"], verbs } },
            ...(people ? [{ rule: { kinds: ["Person"], verbs: ["read"] } }] : []),
          ],
        });
      }
      if (request.method !== "GET") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        sent.push({ method: request.method, path, dryRun, body: await request.json() });
        return reply(dryRun ? 200 : 202, dryRun ? { valid: true, verdict: { ok: true } } : CHANGE);
      }
      if (path === "/api/v1/organization/people") {
        const someone = (email: string) => ({ id: idOf(email), email, firstName: "", lastName: "", enabled: email !== "gone@hel.fi", emailVerified: true, requiredActions: [] });
        const search = url.searchParams.get("search") ?? "";
        return reply(200, { items: PEOPLE.filter((email) => email.includes(search)).slice(0, 5).map(someone) });
      }
      if (path === "/api/v1/projects") return reply(200, list([{ name: "helsinki" }]));
      if (path === "/api/v1/projects/org/changes") return reply(200, list(open));
      if (path === "/api/v1/projects/org/groups/stewards") return reply(200, GROUP);
      if (path === "/api/v1/projects/org/rolebindings") return reply(200, list(ckan ? [...BINDINGS, CKAN_BINDING] : BINDINGS));
      if (path === "/api/v1/projects/org/roles") {
        return reply(200, list([{ apiVersion: V, kind: "Role", metadata: { name: "viewer", namespace: "org" } }, ...(ckan ? [CKAN_ROLE] : [])]));
      }
      if (path === "/api/v1/projects/helsinki/ckaninstances") {
        const instance = (name: string) => ({ apiVersion: V, kind: "CkanInstance", metadata: { name, namespace: "helsinki" }, spec: {} });
        return reply(200, list([instance("hel-fi"), instance("open-data-2")]));
      }
      if (path === "/api/v1/projects/helsinki/roles") {
        return reply(
          200,
          list([
            { apiVersion: V, kind: "Role", metadata: { name: "viewer", namespace: "org" } },
            { apiVersion: V, kind: "Role", metadata: { name: "air-editor", namespace: "helsinki" } },
          ]),
        );
      }
      if (path === "/api/v1/projects/helsinki/apps") return reply(200, list([APP]));
      return undefined;
    },
  });
  return { sent, proposals: () => sent.filter((s) => !s.dryRun) };
}

const section = async (heading: string) =>
  (await screen.findByRole("heading", { name: heading })).closest("section") as HTMLElement;

describe("the group model", () => {
  const manifests = BINDINGS as unknown as Manifest[];

  it("finds the bindings and the application roles naming a group", () => {
    expect(bindingsNaming(manifests, "stewards").map((b) => b.metadata.name)).toEqual(["mixed", "stewards-only"]);
    expect(appRolesNaming([{ project: "helsinki", app: APP as unknown as Manifest }], "stewards")).toEqual([
      { project: "helsinki", app: "alerts", role: "viewer" },
    ]);
    expect(appRolesNaming([], "stewards")).toEqual([]);
  });

  it("takes the group out of a binding, and empties none", () => {
    const edited = bindingWithoutGroup(manifests[0], "stewards");
    expect(edited?.spec).toEqual({ subjects: [{ user: "jana@hel.fi" }], role: "steward", scope: { project: "helsinki" } });
    expect(bindingWithoutGroup(manifests[1], "stewards"), "nobody left: the binding goes").toBeNull();
  });

  // PF-69: an organization role anywhere; a project role in its own project or one of its spaces.
  it("refuses a project role at organization scope or in another project", () => {
    const own = { name: "air-editor", home: "helsinki" };
    expect(refusedPlace({ name: "viewer", home: "org" }, { kind: "organization" })).toBeNull();
    expect(refusedPlace(own, { kind: "organization" })).toBe("projectRoleAtOrganization");
    expect(refusedPlace(own, { kind: "project", project: "tampere" })).toBe("projectRoleElsewhere");
    expect(refusedPlace(own, { kind: "space", project: "helsinki", space: "air" })).toBeNull();
  });

  it("writes the binding for the place chosen", () => {
    expect(groupBinding("stewards", "viewer", { kind: "organization" }, "hel").spec).toEqual({
      subjects: [{ group: "stewards" }],
      role: "viewer",
      scope: { organization: "hel" },
    });
    expect(groupBinding("stewards", "air-editor", { kind: "space", project: "helsinki", space: "air" }, "hel").spec)
      .toMatchObject({ scope: { contextSpace: "air" } });
  });

  it("adds a member once", () => {
    expect(withGroupMember([{ user: "a@hel.fi" }], "b@hel.fi")).toEqual([{ user: "a@hel.fi" }, { user: "b@hel.fi" }]);
    expect(withGroupMember([{ user: "a@hel.fi" }], "a@hel.fi")).toBeNull();
    expect(withGroupMembers([{ user: "a@hel.fi" }], ["b@hel.fi", "a@hel.fi", "c@hel.fi", "b@hel.fi"])).toEqual([
      { user: "a@hel.fi" },
      { user: "b@hel.fi" },
      { user: "c@hel.fi" },
    ]);
    expect(withGroupMembers([{ user: "a@hel.fi" }], ["a@hel.fi"])).toBeNull();
  });

  it("reads a pasted list one e-mail at a time and keeps what is not one", () => {
    expect(emailsIn(" A@hel.fi, b@hel.fi;\nc@hel.fi\ta@hel.fi jana petra@ ")).toEqual({
      emails: ["a@hel.fi", "b@hel.fi", "c@hel.fi"],
      invalid: ["jana", "petra@"],
    });
    expect(emailsIn("  ")).toEqual({ emails: [], invalid: [] });
  });
});

describe("a group's page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the members, the platform roles and the application roles", async () => {
    renderGroup();
    const members = await section(en.access.groupPage.members);
    expect(within(members).getByText(idOf("jana@hel.fi"))).toBeInTheDocument();
    const platform = await section(en.access.groupPage.platformRoles);
    expect(await within(platform).findByText("steward, Project helsinki")).toBeInTheDocument();
    expect(within(platform).getByText(en.access.groupPage.onlySubject)).toBeInTheDocument();
    const apps = await section(en.access.groupPage.appRoles);
    expect(await within(apps).findByRole("link", { name: "viewer in alerts (helsinki)" })).toBeInTheDocument();
    await expectNoViolations(members);
  });

  // T-4949, UI-16: every field says what to enter, with an example, wired to the field.
  it("gives every field a hint with an example", async () => {
    const user = userEvent.setup();
    renderGroup();
    const g = en.access.groupPage;
    const members = await section(g.members);
    expect(within(members).getByLabelText(g.email)).toHaveAccessibleDescription(g.hint.email);
    const platform = await section(g.platformRoles);
    expect(within(platform).getByLabelText(g.where)).toHaveAccessibleDescription(g.hint.where);
    expect(within(platform).getByLabelText(g.role)).toHaveAccessibleDescription(g.hint.role);
    await user.selectOptions(within(platform).getByLabelText(g.where), "helsinki");
    expect(within(platform).getByLabelText(g.space)).toHaveAccessibleDescription(g.hint.space);
    const apps = await section(g.appRoles);
    expect(within(apps).getByLabelText(g.project)).toHaveAccessibleDescription(g.hint.project);
    expect(within(apps).getByLabelText(g.app)).toHaveAccessibleDescription(g.hint.app);
    expect(within(apps).getByLabelText(g.role)).toHaveAccessibleDescription(g.hint.appRole);
    for (const hint of [g.hint.email, g.hint.where, g.hint.role, g.hint.space, g.hint.project, g.hint.app, g.hint.appRole]) {
      expect(hint).toMatch(/e\.g\. /);
    }
  });

  it("lists the catalogues the group's bindings let it manage, and none without one", async () => {
    renderGroup({ ckan: true });
    const reached = await section(en.access.groupPage.ckanTitle);
    expect(await within(reached).findByRole("link", { name: "helsinki / hel-fi" })).toHaveAttribute("href", "/projects/helsinki/ckan");
    expect(within(reached).queryByText("helsinki / open-data-2")).toBeNull();
    expect(within(reached).getByText("through the role ckan-admin-helsinki-hel-fi")).toBeInTheDocument();
  });

  it("says the group manages no catalogue when no binding reaches one", async () => {
    renderGroup();
    const reached = await section(en.access.groupPage.ckanTitle);
    expect(await within(reached).findByText(en.access.groupPage.noCkan)).toBeInTheDocument();
  });

  it("offers the realm's people who are not members yet", async () => {
    const user = userEvent.setup();
    renderGroup({ people: true });
    const members = await section(en.access.groupPage.members);
    // The member field is the shared people picker (T-4947): a combobox over a listbox.
    await user.click(await within(members).findByRole("combobox", { name: en.access.groupPage.email }));
    const offered = within(await screen.findByRole("listbox")).getAllByRole("option").map((option) => option.textContent ?? "");
    // jana is a member already; a disabled person is not offered.
    expect(offered.some((text) => text.includes("petra@hel.fi"))).toBe(true);
    expect(offered.some((text) => text.includes("jana@hel.fi"))).toBe(false);
  });

  it("names a member by a pasted id when the caller may not list people, and never by an address", async () => {
    const user = userEvent.setup();
    renderGroup();
    const members = await section(en.access.groupPage.members);
    const box = within(members).getByRole("combobox", { name: en.access.groupPage.email });
    await user.click(box);
    expect(await screen.findByText(en.picker.person.unlisted)).toBeInTheDocument();
    await user.type(box, "petra@hel.fi");
    expect(await screen.findByText(i18n.t("picker.person.notInvited", { email: "petra@hel.fi" }))).toBeInTheDocument();
    await user.clear(box);
    await user.type(box, idOf("petra@hel.fi"));
    expect(await screen.findByText(i18n.t("picker.person.useId", { id: idOf("petra@hel.fi") }))).toBeInTheDocument();
  });

  it("adds a member as the Group manifest with that one member more, metadata kept", async () => {
    const user = userEvent.setup();
    const { proposals } = renderGroup();
    const members = await section(en.access.groupPage.members);
    await user.type(within(members).getByLabelText(en.access.groupPage.email), `${idOf("petra@hel.fi")}{Enter}`);
    await user.click(within(members).getByRole("button", { name: en.access.groupPage.addMember }));

    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
    const { method, path, body } = proposals()[0];
    expect(`${method} ${path}`).toBe("PUT /api/v1/projects/org/groups/stewards");
    expect(body).toEqual({
      ...GROUP,
      spec: { ...GROUP.spec, members: [{ user: idOf("jana@hel.fi") }, { user: idOf("petra@hel.fi") }] },
    });
  });

  // T-5605: a department of 20 is one Change, not 20.
  it("adds twenty pasted people as one Change naming every one of them", async () => {
    const user = userEvent.setup();
    const { proposals } = renderGroup({ people: true });
    const members = await section(en.access.groupPage.members);
    const twenty = Array.from({ length: 20 }, (_, index) => `person${index + 1}@hel.fi`);
    const box = within(members).getByRole("combobox", { name: en.access.groupPage.email });
    await user.click(box);
    await user.paste([...twenty.slice(0, 10), "Jana@hel.fi"].join(", ") + "\n" + twenty.slice(10).join("; "));
    await user.click(await screen.findByRole("option", { name: i18n.t("picker.person.addList", { count: 21 }) }));
    const chosen = within(members).getByRole("list", { name: i18n.t("picker.chosen", { label: en.access.groupPage.email }) });
    // Each address is looked up and written as its person's id; jana is a member already, so
    // she is not chosen twice.
    await vi.waitFor(() => expect(within(chosen).getAllByRole("listitem")).toHaveLength(20));
    await user.click(within(members).getByRole("button", { name: i18n.t("access.groupPage.addMembers", { count: 20 }) }));

    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
    const { method, path, body } = proposals()[0];
    expect(`${method} ${path}`).toBe("PUT /api/v1/projects/org/groups/stewards");
    expect((body as { spec: GroupSpec }).spec.members).toEqual([
      { user: idOf("jana@hel.fi") },
      ...twenty.map((email) => ({ user: idOf(email) })),
    ]);
    expect((body as { metadata: unknown }).metadata).toEqual(GROUP.metadata);
  });

  it("names every pasted word that is not an e-mail and proposes nothing for it", async () => {
    const user = userEvent.setup();
    const { sent } = renderGroup();
    const members = await section(en.access.groupPage.members);
    const box = within(members).getByRole("combobox", { name: en.access.groupPage.email });
    await user.click(box);
    await user.paste("jana.novak, petra@");
    await user.keyboard("{Enter}");
    const named = i18n.t("access.groupPage.notEmails", { words: "jana.novak, petra@" });
    expect(await within(members).findByText(named)).toBeInTheDocument();
    expect(box).toHaveAttribute("aria-invalid", "true");
    expect(box).toHaveAccessibleDescription(expect.stringContaining(named));
    const add = within(members).getByRole("button", { name: en.access.groupPage.addMember });
    expect(add).toHaveAttribute("aria-disabled", "true");
    await user.click(add);
    expect(sent).toEqual([]);
  });

  it("holds every member edit while a Change on the group waits, and links it", async () => {
    const user = userEvent.setup();
    const { sent } = renderGroup({ open: [OPEN] });
    const members = await section(en.access.groupPage.members);
    const waiting = i18n.t("access.groupPage.waiting", { change: "chg-0000001f" });
    expect(await within(members).findByRole("link", { name: i18n.t("access.groupPage.openChange", { change: "chg-0000001f" }) }))
      .toHaveAttribute("href", "/projects/org/approvals/chg-0000001f");
    await user.type(within(members).getByLabelText(en.access.groupPage.email), `${idOf("petra@hel.fi")}{Enter}`);
    for (const name of [en.access.groupPage.addMember, i18n.t("access.groupPage.removeMember", { member: idOf("jana@hel.fi") })]) {
      const button = within(members).getByRole("button", { name });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).toHaveAccessibleDescription(expect.stringContaining(waiting));
      await user.click(button);
    }
    expect(sent).toEqual([]);
    await expectNoViolations(members);
  });

  it("lets a Change on another group or a decided one not hold the members", async () => {
    const user = userEvent.setup();
    const other = { ...OPEN, summary: { ...OPEN.summary, params: { kind: "Group", name: "editors", fields: 1 } } };
    const decided = { ...OPEN, status: { ...OPEN.status, phase: "Merged" } };
    const { proposals } = renderGroup({ open: [other, decided] });
    const members = await section(en.access.groupPage.members);
    await user.type(within(members).getByLabelText(en.access.groupPage.email), `${idOf("petra@hel.fi")}{Enter}`);
    await user.click(within(members).getByRole("button", { name: en.access.groupPage.addMember }));
    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
  });

  it("takes the group out of a binding that names someone else too", async () => {
    const user = userEvent.setup();
    const { proposals } = renderGroup();
    const platform = await section(en.access.groupPage.platformRoles);
    await user.click(
      await within(platform).findByRole("button", { name: "Take the group out of steward, Project helsinki" }),
    );
    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
    expect(proposals()[0].path).toBe("/api/v1/projects/org/rolebindings/mixed");
    expect((proposals()[0].body as { spec: unknown }).spec).toEqual({
      subjects: [{ user: "jana@hel.fi" }],
      role: "steward",
      scope: { project: "helsinki" },
    });
  });

  // PF-69: the project role stays in the list and the button says why it cannot be given here.
  it("refuses a project role for the whole organization and says why", async () => {
    const user = userEvent.setup();
    const { proposals } = renderGroup();
    const platform = await section(en.access.groupPage.platformRoles);
    const role = within(platform).getByLabelText(en.access.groupPage.role);
    await within(platform).findByRole("option", { name: "air-editor (role of project helsinki)" });
    await user.selectOptions(role, "helsinki/air-editor");
    const give = within(platform).getByRole("button", { name: en.access.groupPage.addRole });
    expectDenied(give, /cannot be given for the whole organization/);

    await user.selectOptions(within(platform).getByLabelText(en.access.groupPage.where), "helsinki");
    expect(give).not.toHaveAttribute("aria-disabled");
    await user.click(give);
    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
    expect(`${proposals()[0].method} ${proposals()[0].path}`).toBe("POST /api/v1/projects/org/rolebindings");
    expect((proposals()[0].body as { spec: unknown }).spec).toEqual({
      subjects: [{ group: "stewards" }],
      role: "air-editor",
      scope: { project: "helsinki" },
    });
  });

  it("gives the group an application role as the App manifest with the group added", async () => {
    const user = userEvent.setup();
    const { proposals } = renderGroup();
    const apps = await section(en.access.groupPage.appRoles);
    await user.selectOptions(within(apps).getByLabelText(en.access.groupPage.project), "helsinki");
    await user.selectOptions(await within(apps).findByLabelText(en.access.groupPage.app), "alerts");
    await user.selectOptions(within(apps).getByLabelText(en.access.groupPage.role), "steward");
    await user.click(within(apps).getByRole("button", { name: en.access.groupPage.addAppRole }));

    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
    expect(proposals()[0].path).toBe("/api/v1/projects/helsinki/apps/alerts");
    expect((proposals()[0].body as typeof APP).spec.access).toEqual([
      { role: "viewer", subjects: [{ group: "stewards" }, { user: "petra@hel.fi" }] },
      { role: "steward", subjects: [{ group: "stewards" }] },
    ]);
  });

  it("takes the group out of an application role", async () => {
    const user = userEvent.setup();
    const { proposals } = renderGroup();
    const apps = await section(en.access.groupPage.appRoles);
    await user.click(
      await within(apps).findByRole("button", { name: "Take the group out of viewer in alerts (helsinki)" }),
    );
    await vi.waitFor(() => expect(proposals()).toHaveLength(1));
    expect((proposals()[0].body as typeof APP).spec.access).toEqual([
      { role: "viewer", subjects: [{ user: "petra@hel.fi" }] },
    ]);
  });

  // UI-44, PF-50: a reader reaches the controls, is told why, and nothing is proposed.
  it("keeps every write control reachable but refused for a caller who may only read", async () => {
    const { proposals } = renderGroup({ verbs: ["read"] });
    const members = await section(en.access.groupPage.members);
    const remove = within(members).getByRole("button", { name: `Remove ${idOf("jana@hel.fi")} from the group` });
    await vi.waitFor(() => expectDenied(remove, "Only an organization administrator can add or change groups."));
    const platform = await section(en.access.groupPage.platformRoles);
    expectDenied(within(platform).getByRole("button", { name: en.access.groupPage.addRole }));
    // T-6423: each card with greyed controls says why in words, not only the Members card.
    expect(within(platform).getByText(/^Only an organization administrator/, { selector: "p" })).toBeVisible();
    const apps = await section(en.access.groupPage.appRoles);
    expect(within(apps).getByText(/^Only an organization administrator/, { selector: "p" })).toBeVisible();
    // A bare "Remove group" is the grid filter's draft control to the walker; this one stays judged.
    expect(screen.getByRole("button", { name: /^Remove group: / })).toBeInTheDocument();
    expect(viewerWriteFindings(false)).toEqual([]);
    expect(proposals()).toHaveLength(0);
  });
});
