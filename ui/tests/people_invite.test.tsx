/**
 * Inviting a person (PF-108, T-3237): the invitation names the project it leads into, the role
 * picker says what each role may do and proposes the chosen one as a RoleBinding after a green
 * check, a failed proposal still reports the person invited, and pending invitations show when
 * their link expires and are sent again or revoked.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/organization/invite.ts.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { People } from "../src/pages/organization/People";
import { INVITE_ROLES, inviteBinding, invitationState } from "../src/pages/organization/invite";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";

const P = en.organization.people;
const NOW = new Date("2026-10-07T12:00:00Z");

const person = (id: string, first: string, extra: Record<string, unknown> = {}) => ({
  id,
  email: `${first.toLowerCase()}@example.org`,
  firstName: first,
  lastName: "Nová",
  enabled: true,
  emailVerified: true,
  requiredActions: [] as string[],
  createdAt: "2026-10-07T08:00:00Z",
  lastSeen: null,
  locale: "en",
  pendingDeletion: null,
  invitationExpires: null as string | null,
  ...extra,
});

const ACCEPTED = person("ada-id", "Ada");
const WAITING = person("nora-id", "Nora", {
  emailVerified: false,
  requiredActions: ["VERIFY_EMAIL", "UPDATE_PASSWORD"],
  invitationExpires: "2099-10-08T08:00:00Z",
});
const LAPSED = person("ema-id", "Ema", {
  emailVerified: false,
  requiredActions: ["VERIFY_EMAIL", "UPDATE_PASSWORD"],
  invitationExpires: "2026-10-01T08:00:00Z",
});

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000042", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

interface Sent {
  method: string;
  path: string;
  search: string;
  body: unknown;
}

const reply = (status: number, body?: unknown) =>
  new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
  });

function answer(sent: Sent[], { bindingRefused = false, memberOf }: { bindingRefused?: boolean; memberOf?: string } = {}) {
  return async (url: URL, request: Request) => {
    const path = url.pathname;
    if (path.endsWith("/permissions/me")) {
      return reply(200, {
        project: "org",
        bootstrap: false,
        grants: [{ rule: { kinds: ["Person"], verbs: ["read", "create", "update", "disable", "delete"] } }],
      });
    }
    if (path === "/api/v1/projects") return reply(200, { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }, { name: "praha" }] });
    const text = request.method === "GET" || request.method === "DELETE" ? "" : await request.text();
    const record = () => sent.push({ method: request.method, path, search: url.search, body: text ? JSON.parse(text) : undefined });
    if (path === "/api/v1/projects/org/rolebindings" && request.method === "POST") {
      record();
      if (bindingRefused) return reply(403, { title: "Forbidden", status: 403, detail: "you may not propose a RoleBinding." });
      return url.search.includes("dryRun") ? reply(200, { valid: true, verdict: { ok: true, findings: [] } }) : reply(202, CHANGE);
    }
    if (!path.startsWith("/api/v1/organization/people")) return undefined;
    record();
    if (request.method === "GET") return reply(200, { items: [ACCEPTED, WAITING, LAPSED] });
    if (request.method === "POST" && path === "/api/v1/organization/people") {
      return reply(201, {
        person: person("00000000-0000-4000-8000-0000000000aa", "Viera", { requiredActions: ["VERIFY_EMAIL"] }),
        emailSent: true,
        ...(memberOf ? { memberOf } : {}),
      });
    }
    if (path.endsWith("/resend-invitation")) return reply(202, { emailSent: true });
    if (request.method === "DELETE") return reply(204);
    return reply(204);
  };
}

async function invite(user: ReturnType<typeof userEvent.setup>, project?: string, role?: string) {
  await user.click(await screen.findByRole("button", { name: P.new }));
  const dialog = await screen.findByRole("dialog", { name: P.newTitle });
  await user.type(within(dialog).getByRole("textbox", { name: new RegExp(P.form.email) }), "viera@example.org");
  await user.type(within(dialog).getByRole("textbox", { name: new RegExp(P.form.firstName) }), "Viera");
  await user.type(within(dialog).getByRole("textbox", { name: new RegExp(P.form.lastName) }), "Nová");
  if (project) {
    const projects = within(dialog).getByRole("combobox", { name: new RegExp(P.invite.project) });
    await waitFor(() => expect(within(projects).getAllByRole("option")).toHaveLength(3));
    await user.selectOptions(projects, project);
  }
  if (role) await user.click(within(dialog).getByRole("radio", { name: role }));
  return dialog;
}

describe("the invitation's rules", () => {
  it("proposes the role in the project, for the person's Keycloak id and never the address (ADR-N-061)", () => {
    const binding = inviteBinding("00000000-0000-4000-8000-0000000000aa", "editor", "helsinki");
    expect(binding.kind).toBe("RoleBinding");
    expect(binding.metadata.namespace).toBe("org");
    expect(binding.spec).toEqual({ subjects: [{ user: "00000000-0000-4000-8000-0000000000aa" }], role: "editor", scope: { project: "helsinki" } });
  });

  it("offers the roles least first", () => {
    expect(INVITE_ROLES).toEqual(["viewer", "editor", "steward"]);
  });

  it("tells an accepted, a pending, an expired and an unrecorded invitation apart", () => {
    expect(invitationState({ requiredActions: [], invitationExpires: "2026-10-01T00:00:00Z" }, NOW)).toEqual({ state: "accepted" });
    expect(invitationState({ requiredActions: ["VERIFY_EMAIL"], invitationExpires: "2026-10-08T00:00:00Z" }, NOW)).toEqual({
      state: "pending",
      expires: new Date("2026-10-08T00:00:00Z"),
    });
    expect(invitationState({ requiredActions: ["VERIFY_EMAIL"], invitationExpires: "2026-10-07T12:00:00Z" }, NOW).state).toBe("expired");
    expect(invitationState({ requiredActions: ["VERIFY_EMAIL"], invitationExpires: null }, NOW)).toEqual({ state: "pending" });
    expect(invitationState({ requiredActions: ["VERIFY_EMAIL"], invitationExpires: "not a date" }, NOW)).toEqual({ state: "pending" });
  });
});

describe("inviting a person into a project (PF-108)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says what each role may do, and offers none before a project is chosen", async () => {
    const user = userEvent.setup();
    renderPage(<People />, { path: "/organization/people", answer: answer([]) });
    await user.click(await screen.findByRole("button", { name: P.new }));
    const dialog = await screen.findByRole("dialog", { name: P.newTitle });
    expect(within(dialog).queryByRole("radiogroup")).toBeNull();

    await user.selectOptions(within(dialog).getByRole("combobox", { name: new RegExp(P.invite.project) }), "helsinki");
    const roles = within(dialog).getByRole("radiogroup", { name: P.invite.role });
    for (const role of INVITE_ROLES) {
      expect(within(roles).getByRole("radio", { name: P.invite.roles[role].label })).toHaveAccessibleDescription(P.invite.roles[role].may);
    }
    expect(within(roles).getByRole("radio", { name: P.invite.roles.viewer.label })).toBeChecked();
    await expectNoViolations(dialog);
  });

  it("names the project in the invitation and proposes the role after a green check", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });
    const dialog = await invite(user, "helsinki", P.invite.roles.editor.label);
    await user.click(within(dialog).getByRole("button", { name: P.create }));

    expect(await screen.findByText(/The link then opens the project helsinki/)).toBeInTheDocument();
    expect(await screen.findByText(/The role Editor for viera@example\.org in helsinki is proposed/)).toBeInTheDocument();
    const created = sent.find((s) => s.method === "POST" && s.path === "/api/v1/organization/people");
    expect(created?.body).toMatchObject({ email: "viera@example.org", project: "helsinki" });
    const bindings = sent.filter((s) => s.path === "/api/v1/projects/org/rolebindings");
    expect(bindings.map((s) => s.search)).toEqual(["?dryRun=All", ""]);
    expect((bindings[1].body as { spec: unknown }).spec).toEqual({
      subjects: [{ user: "00000000-0000-4000-8000-0000000000aa" }],
      role: "editor",
      scope: { project: "helsinki" },
    });
  });

  it("invites into the organization alone without proposing anything", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });
    const dialog = await invite(user);
    await user.click(within(dialog).getByRole("button", { name: P.create }));

    await screen.findByText(/sent viera@example\.org an e-mail/);
    const created = sent.find((s) => s.method === "POST" && s.path === "/api/v1/organization/people");
    expect(created?.body).not.toHaveProperty("project");
    expect(sent.filter((s) => s.path.includes("rolebindings"))).toEqual([]);
  });

  it("says a person invited with no project is a member of the organization (ADR-N-067 §2.5, T-6412)", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { memberOf: "helsinki" }) });
    const dialog = await invite(user);
    await user.click(within(dialog).getByRole("button", { name: P.create }));

    await screen.findByText(/They are a member of helsinki with no project/);
    expect(sent.filter((s) => s.path.includes("rolebindings"))).toEqual([]);
  });

  it("still reports the person invited when the role cannot be proposed, with the reason", async () => {
    const user = userEvent.setup();
    renderPage(<People />, { path: "/organization/people", answer: answer([], { bindingRefused: true }) });
    const dialog = await invite(user, "praha", P.invite.roles.steward.label);
    await user.click(within(dialog).getByRole("button", { name: P.create }));

    expect(await screen.findByText(/The link then opens the project praha/)).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(/the role Steward in praha was not proposed: you may not propose a RoleBinding\./);
  });
});

describe("pending invitations (PF-108)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists only the invitations not accepted, with when each link expires", async () => {
    renderPage(<People />, { path: "/organization/people", answer: answer([]) });
    const pending = await screen.findByRole("region", { name: P.pending.title });
    const items = within(pending).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("nora@example.org");
    expect(items[0]).toHaveTextContent(/The link works until .*2099/);
    expect(items[1]).toHaveTextContent("ema@example.org");
    expect(items[1]).toHaveTextContent(/The link expired on Oct 1, 2026, .*\. Send it again\./);
    expect(within(pending).queryByText("ada@example.org")).toBeNull();
    await expectNoViolations(pending);
  });

  it("sends an invitation again after asking", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });
    await user.click(await screen.findByRole("button", { name: "Send the invitation to ema@example.org again" }));
    const ask = await screen.findByRole("alertdialog");
    await user.click(within(ask).getByRole("button", { name: P.action["resend-invitation"] }));

    expect(await screen.findByText("The realm sent the invitation to ema@example.org again, with a new link.")).toBeInTheDocument();
    expect(sent.filter((s) => s.method === "POST").map((s) => s.path)).toEqual(["/api/v1/organization/people/ema-id/resend-invitation"]);
  });

  it("revokes an invitation by deleting the person, and only after asking", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });
    await user.click(await screen.findByRole("button", { name: "Revoke the invitation of nora@example.org" }));
    const ask = await screen.findByRole("alertdialog");
    expect(sent.filter((s) => s.method === "DELETE")).toEqual([]);
    await user.click(within(ask).getByRole("button", { name: P.pending.revoke }));

    expect(await screen.findByText("The invitation of nora@example.org is revoked and the account deleted.")).toBeInTheDocument();
    expect(sent.filter((s) => s.method === "DELETE").map((s) => s.path)).toEqual(["/api/v1/organization/people/nora-id"]);
  });
});
