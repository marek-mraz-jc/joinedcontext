/**
 * An application's roles and who holds them, on its App page (T-2595, T-2687, AP-99, AP-119,
 * ADR-N-031): each role with its description, its default group `{app}-{role}` and that group's
 * members, and the other groups holding it. Add person and Remove propose the default group; Give
 * this role to a group and Remove propose the App; both through the checked propose, and every
 * write control disabled with the reason for a person who may not propose an App (PF-50, UI-44).
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { asUser, othersOf, withMember, withoutMember } from "../src/pages/apps/members";
import type { RolesSpec } from "../src/pages/apps/members";
import { RolesAndMembers } from "../src/pages/apps/RolesAndMembers";
import { expectNoViolations } from "./checks";
import { list, renderPage } from "./page_contract";

const APP = "/api/v1/projects/helsinki/apps/alerts";
const GROUPS = "/api/v1/projects/org/groups";
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-42", namespace: "helsinki" },
  spec: { lane: "red" },
  status: { phase: "Pending" },
};

function manifest(spec: Partial<RolesSpec> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { name: "alerts", namespace: "helsinki", title: "Alerts", labels: { team: "ops" } },
    spec: {
      kind: "static",
      visibility: "roles",
      roles: [
        { name: "viewer", title: { en: "Viewer" }, description: "Reads the alerts" },
        { name: "steward", title: { en: "Steward" }, description: "Writes the steward's note" },
      ],
      access: [
        { role: "viewer", subjects: [{ group: "alerts-viewer" }, { group: "helsinki-operations" }, { user: "petra@hel.fi" }] },
        { role: "steward", subjects: [{ group: "alerts-steward" }] },
      ],
      dataNeeds: [],
      ...spec,
    },
  };
}

function defaultGroup(name: string, members: string[]) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Group",
    metadata: { name, namespace: "org", annotations: { "joinedcontext.com/app": "helsinki/alerts" } },
    spec: { members: members.map((user) => ({ user })) },
  };
}

function renderSection({
  app = manifest() as unknown,
  mayPropose = true,
  groupsReadable = true,
}: { app?: unknown; mayPropose?: boolean; groupsReadable?: boolean } = {}) {
  const sent: { url: string; dryRun: boolean; body: unknown }[] = [];
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
    });
  const stored: Record<string, unknown> = {
    "alerts-viewer": defaultGroup("alerts-viewer", ["eva@hel.fi"]),
    "alerts-steward": defaultGroup("alerts-steward", []),
  };
  const view = renderPage(<RolesAndMembers project="helsinki" name="alerts" />, {
    path: "/projects/helsinki/apps/alerts",
    answer: async (url, request) => {
      if (url.pathname === "/api/v1/projects/helsinki/permissions/me") {
        return reply(200, {
          project: "helsinki",
          bootstrap: false,
          grants: [{ rule: { kinds: ["App"], verbs: mayPropose ? ["read", "propose"] : ["read"] } }],
        });
      }
      if (url.pathname === GROUPS) {
        return groupsReadable
          ? reply(200, list([{ metadata: { name: "helsinki-operations" } }, { metadata: { name: "alert-editors" } }]))
          : reply(404, { title: "Not Found", status: 404 });
      }
      const group = url.pathname.startsWith(`${GROUPS}/`) ? url.pathname.slice(GROUPS.length + 1) : null;
      if (group !== null && request.method === "GET") {
        return group in stored ? reply(200, stored[group]) : reply(404, { title: "Not Found", status: 404 });
      }
      if (url.pathname === APP && request.method === "GET") return reply(200, app);
      if ((url.pathname === APP || group !== null) && request.method === "PUT") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        sent.push({ url: url.pathname, dryRun, body: await request.json() });
        return reply(200, dryRun ? { valid: true, verdict: { ok: true } } : CHANGE);
      }
      return undefined;
    },
  });
  return { sent, view };
}

async function section(): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", { name: en.apps.roles.title });
  return heading.closest("section") as HTMLElement;
}

async function role(name: string): Promise<HTMLElement> {
  return within(await section()).getByRole("listitem", { name });
}

describe("an application's roles and members", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  // AP-119: every role with its default group and members, and the others holding it.
  it("lists each role with its default group, its members and the other holders", async () => {
    renderSection();
    const viewer = await role("Viewer");
    expect(within(viewer).getByText("Reads the alerts")).toBeInTheDocument();
    expect(within(viewer).getByRole("heading", { name: "Default group alerts-viewer" })).toBeInTheDocument();
    expect(await within(viewer).findByText("eva@hel.fi")).toBeInTheDocument();
    expect(within(viewer).getByText("Group helsinki-operations")).toBeInTheDocument();
    expect(within(viewer).getByText("petra@hel.fi")).toBeInTheDocument();
    expect(within(viewer).queryByText("Group alerts-viewer")).toBeNull();
    expect(within(viewer).getByText("(Members: 3)")).toBeInTheDocument();
    const steward = await role("Steward");
    expect(await within(steward).findByText(en.apps.roles.nobody)).toBeInTheDocument();
    expect(within(steward).getByText(en.apps.roles.noOthers)).toBeInTheDocument();
    await expectNoViolations(await section());
  });

  // AP-119, PF-57: Add person checks, then proposes the default group with the person added,
  // its annotation kept, and the App untouched.
  it("adds a person by proposing the default group", async () => {
    const user = userEvent.setup();
    const { sent } = renderSection();
    const steward = await role("Steward");
    await within(steward).findByText(en.apps.roles.nobody);
    // A caller who may not list people pastes the person's id; an address alone names nobody (ADR-N-061).
    const box = within(steward).getByRole("combobox", { name: en.apps.roles.email });
    await user.type(box, "demo.viewer@hel.fi{Enter}");
    expect(within(steward).getByRole("button", { name: en.apps.roles.addPerson })).toHaveAttribute("aria-disabled", "true");
    await user.clear(box);
    await user.type(box, "00000000-0000-4000-8000-0000000000dd{Enter}");
    await user.click(within(steward).getByRole("button", { name: en.apps.roles.addPerson }));

    expect(await screen.findByRole("link", { name: /review/i })).toBeInTheDocument();
    expect(sent.map((s) => [s.url, s.dryRun])).toEqual([
      [`${GROUPS}/alerts-steward`, true],
      [`${GROUPS}/alerts-steward`, false],
    ]);
    expect(sent[1].body).toEqual({
      ...defaultGroup("alerts-steward", []),
      spec: { members: [{ user: "00000000-0000-4000-8000-0000000000dd" }] },
    });
  });

  // AP-119: Remove on a default group member proposes the group without them.
  it("removes a person by proposing the default group", async () => {
    const user = userEvent.setup();
    const { sent } = renderSection();
    const viewer = await role("Viewer");
    await user.click(await within(viewer).findByRole("button", { name: "Remove eva@hel.fi from Viewer" }));
    await screen.findByRole("link", { name: /review/i });
    expect(sent[1].url).toBe(`${GROUPS}/alerts-viewer`);
    expect((sent[1].body as ReturnType<typeof defaultGroup>).spec.members).toEqual([]);
  });

  // AP-119, AP-91: giving the role to another group proposes the App with the new access entry.
  it("gives the role to another group by proposing the App", async () => {
    const user = userEvent.setup();
    const { sent } = renderSection();
    const steward = await role("Steward");
    const picker = within(steward).getByLabelText(en.apps.roles.group);
    await vi.waitFor(() => expect(within(picker).getByRole("option", { name: "alert-editors" })).toBeInTheDocument());
    await user.selectOptions(picker, "alert-editors");
    await user.click(within(steward).getByRole("button", { name: en.apps.roles.give }));

    await screen.findByRole("link", { name: /review/i });
    expect(sent.map((s) => [s.url, s.dryRun])).toEqual([
      [APP, true],
      [APP, false],
    ]);
    const body = sent[1].body as ReturnType<typeof manifest>;
    expect(body.metadata).toEqual(manifest().metadata);
    expect(body.spec.access[1]).toEqual({
      role: "steward",
      subjects: [{ group: "alerts-steward" }, { group: "alert-editors" }],
    });
    expect(body.spec.dataNeeds).toEqual([]);
  });

  // AP-119: Remove on another group proposes the App without it, the default group kept.
  it("takes the role from another group by proposing the App", async () => {
    const user = userEvent.setup();
    const { sent } = renderSection();
    const viewer = await role("Viewer");
    await user.click(within(viewer).getByRole("button", { name: "Remove Group helsinki-operations from Viewer" }));
    await screen.findByRole("link", { name: /review/i });
    expect(sent[1].url).toBe(APP);
    expect((sent[1].body as ReturnType<typeof manifest>).spec.access[0]).toEqual({
      role: "viewer",
      subjects: [{ group: "alerts-viewer" }, { user: "petra@hel.fi" }],
    });
  });

  // UI-44, PF-50: a reader reaches every write control, is told why, and nothing is sent.
  it("keeps every write control reachable but refused, with the reason, for a reader", async () => {
    const user = userEvent.setup();
    const { sent } = renderSection({ mayPropose: false });
    const viewer = await role("Viewer");
    const remove = await within(viewer).findByRole("button", { name: "Remove eva@hel.fi from Viewer" });
    await vi.waitFor(() => expect(remove).toHaveAttribute("aria-disabled", "true"));
    for (const name of [en.apps.roles.addPerson, en.apps.roles.give, "Remove Group helsinki-operations from Viewer"]) {
      expect(within(viewer).getByRole("button", { name })).toHaveAttribute("aria-disabled", "true");
    }
    expect(within(viewer).getAllByText(/propose/).length).toBeGreaterThan(0);
    await user.click(remove);
    expect(sent).toEqual([]);
  });

  // A default group not there yet, and groups the person may not list, each say why.
  it("says why a person or a group cannot be added", async () => {
    renderSection({
      groupsReadable: false,
      app: manifest({ roles: [{ name: "editor", title: { en: "Editor" } }], access: [] }),
    });
    const editor = await role("Editor");
    const notYet = en.apps.roles.noDefaultGroup.replace("{group}", "alerts-editor");
    expect(await within(editor).findAllByText(notYet)).not.toHaveLength(0);
    expect(within(editor).getByRole("button", { name: en.apps.roles.addPerson })).toHaveAttribute("aria-disabled", "true");
    await vi.waitFor(() =>
      expect(within(editor).getByText(en.apps.roles.groupsUnreadable)).toBeInTheDocument(),
    );
  });

  // An app without roles has nothing to manage, and an unreadable one draws nothing.
  it("draws nothing for an app without roles or one the person may not read", async () => {
    const { view } = renderSection({ app: manifest({ roles: [], access: [] }) });
    await vi.waitFor(() => expect(view.container.innerHTML).toBe(""));
    expect(screen.queryByRole("heading", { name: en.apps.roles.title })).toBeNull();
  });
});

describe("the access a change proposes", () => {
  const spec: RolesSpec = { roles: [{ name: "viewer" }], access: [{ role: "viewer", subjects: [{ user: "a@hel.fi" }] }] };

  // AP-91: a member is added once; a role emptied of members loses its entry, not its declaration.
  it("adds once, removes to nothing, and keeps the role declared", () => {
    expect(withMember(spec, "viewer", { user: "a@hel.fi" })).toBeNull();
    expect(withMember(spec, "viewer", { group: "a" })?.access).toEqual([
      { role: "viewer", subjects: [{ user: "a@hel.fi" }, { group: "a" }] },
    ]);
    const emptied = withoutMember(spec, "viewer", { user: "a@hel.fi" });
    expect(emptied.access).toEqual([]);
    expect(emptied.roles).toEqual(spec.roles);
  });

  // AP-119: the default group is not among the others, whoever else holds the role is.
  it("keeps the default group out of the others", () => {
    const held: RolesSpec = { access: [{ role: "viewer", subjects: [{ group: "x-viewer" }, { group: "ops" }, { user: "a@hel.fi" }] }] };
    expect(othersOf(held, "viewer", "x-viewer")).toEqual([{ group: "ops" }, { user: "a@hel.fi" }]);
    expect(othersOf(held, "steward", "x-steward")).toEqual([]);
  });

  // AP-91: a person is an e-mail, lower case; anything else is not one.
  it("reads a typed e-mail and refuses what is not one", () => {
    expect(asUser("  Jana@Hel.FI ")).toBe("jana@hel.fi");
    for (const typed of ["", "jana", "jana@", "@hel.fi", "ja na@hel.fi", "*"]) {
      expect(asUser(typed)).toBeNull();
    }
  });
});
