/**
 * Organization → People (T-2684, PF-90…PF-94, UI-44, UI-75): the searchable table, the new-person
 * form's validation, each lifecycle action behind its confirm dialog, controls a caller may not
 * use disabled with the reason, and the one-time temporary password (PF-92) shown once and gone
 * from the page, storage, the address and the console once its dialog closes.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/organization/People.tsx.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { changedFields, People, PersonPage } from "../src/pages/organization/People";
import { expectDenied, expectNoViolations, expectOpen } from "./checks";
import { renderPage } from "./page_contract";

const P = en.organization.people;
const PASSWORD = "Tq7#vR2m!Kx9$pL4wZ8@nB3e";

const person = (id: string, first: string, last: string, extra: Record<string, unknown> = {}) => ({
  id,
  email: `${first.toLowerCase()}@hel.fi`,
  firstName: first,
  lastName: last,
  enabled: true,
  emailVerified: true,
  requiredActions: [],
  createdAt: "2026-09-01T08:00:00Z",
  lastSeen: null,
  locale: "sk",
  pendingDeletion: null,
  ...extra,
});

const JANA = person("jana-id", "Jana", "Nováková", { lastSeen: "2026-09-24T10:30:00Z" });
const EVA = person("eva-id", "Eva", "Kováčová", { enabled: false, emailVerified: false });

const DETAIL = {
  person: JANA,
  groups: [{ name: "stewards" }],
  platformRoles: [
    { role: "viewer", binding: "readers", scope: { organization: "hel" }, via: { group: "stewards" } },
    { role: "steward", binding: "mixed", scope: { project: "helsinki" }, via: { user: "jana@hel.fi" } },
  ],
  appRoles: [{ project: "helsinki", app: "alerts", role: "viewer", via: { group: "stewards" } }],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000009", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 1 } },
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

function answer(
  sent: Sent[],
  {
    verbs = ["read", "create", "update", "disable", "delete"],
    smtp = true,
    next = null as number | null,
    refuseEva = false,
  } = {},
) {
  return async (url: URL, request: Request) => {
    const path = url.pathname;
    if (path.endsWith("/permissions/me")) {
      return reply(200, { project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Person"], verbs } }] });
    }
    if (!path.startsWith("/api/v1/organization/people")) return undefined;
    const text = request.method === "GET" || request.method === "DELETE" ? "" : await request.text();
    sent.push({ method: request.method, path, search: url.search, body: text ? JSON.parse(text) : undefined });
    if (request.method === "GET" && path === "/api/v1/organization/people") {
      return reply(200, { items: [JANA, EVA], ...(next === null ? {} : { next }) });
    }
    if (request.method === "GET") return reply(200, DETAIL);
    if (request.method === "POST" && path === "/api/v1/organization/people") {
      return reply(201, {
        person: person("new-id", "Nora", "Nová"),
        emailSent: smtp,
        ...(smtp ? {} : { temporaryPassword: PASSWORD }),
      });
    }
    if (path.endsWith("/reset-password")) {
      return reply(smtp ? 202 : 200, { emailSent: smtp, ...(smtp ? {} : { temporaryPassword: PASSWORD }) });
    }
    if (path.endsWith("/eva-id/disable") && refuseEva) {
      return reply(403, { title: "Forbidden", status: 403, detail: "eva@hel.fi holds a right you do not" });
    }
    if (path.endsWith("/disable")) return reply(200, { ...JANA, enabled: false });
    if (path.endsWith("/people/remove")) return reply(202, CHANGE);
    if (request.method === "DELETE") return reply(202, CHANGE);
    return reply(204);
  };
}

/** Nothing but the dialog ever held the password. */
function expectPasswordGone(log: ReturnType<typeof vi.spyOn>[]) {
  expect(document.body.innerHTML).not.toContain(PASSWORD);
  expect(JSON.stringify({ ...window.localStorage })).not.toContain(PASSWORD);
  expect(JSON.stringify({ ...window.sessionStorage })).not.toContain(PASSWORD);
  expect(window.location.href).not.toContain(PASSWORD);
  for (const spy of log) {
    expect(JSON.stringify(spy.mock.calls)).not.toContain(PASSWORD);
  }
}

describe("Organization → People", () => {
  let log: ReturnType<typeof vi.spyOn>[] = [];

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    log = (["log", "info", "warn", "error", "debug"] as const).map((level) => vi.spyOn(console, level));
  });

  afterEach(() => {
    vi.restoreAllMocks();
    window.localStorage.clear();
    window.sessionStorage.clear();
  });

  it("lists the people with their state and last activity, searched and paged", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { next: 50 }) });

    await screen.findByRole("link", { name: "Jana Nováková" });
    const table = screen.getByRole("table", { name: P.caption });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByRole("link", { name: "Jana Nováková" })).toHaveAttribute(
      "href",
      "/organization/people/jana-id",
    );
    expect(rows[0]).toHaveTextContent("jana@hel.fi");
    expect(rows[0]).toHaveTextContent(P.enabled);
    expect(rows[0]).toHaveTextContent("2026");
    expect(rows[1]).toHaveTextContent(P.disabled);
    expect(rows[1]).toHaveTextContent(P.unverified);
    expect(rows[1]).toHaveTextContent(P.noSession);

    await user.type(screen.getByRole("searchbox", { name: P.search }), "nov");
    await user.click(screen.getByRole("button", { name: P.searchButton }));
    await waitFor(() => expect(sent.some((s) => s.search.includes("search=nov"))).toBe(true));

    await user.click(screen.getByRole("button", { name: P.next }));
    await waitFor(() => expect(sent.some((s) => s.search.includes("first=50"))).toBe(true));
    await expectNoViolations(screen.getByRole("table", { name: P.caption }).closest("section") as HTMLElement);
  });

  it("fetches nobody for a caller whose role does not read people, and says who can", async () => {
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { verbs: ["update"] }) });

    expect(await screen.findByText(P.hidden)).toBeInTheDocument();
    expectDenied(screen.getByRole("button", { name: P.new }), "Only an organization administrator can add people.");
    expect(sent).toEqual([]);
  });

  it("validates the new-person form before anything is sent, then invites", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });

    await user.click(await screen.findByRole("button", { name: P.new }));
    const dialog = await screen.findByRole("dialog", { name: P.newTitle });
    await user.click(within(dialog).getByRole("button", { name: P.create }));
    expect(within(dialog).getByRole("textbox", { name: new RegExp(P.form.email) })).toHaveAccessibleDescription(
      new RegExp(P.form.emailInvalid.replace(/[.]/g, "\\.")),
    );
    expect(within(dialog).getAllByText(P.form.required)).toHaveLength(2);
    expect(sent.filter((s) => s.method === "POST")).toEqual([]);

    await user.type(within(dialog).getByRole("textbox", { name: new RegExp(P.form.email) }), " Nora@Example.org ");
    await user.type(within(dialog).getByRole("textbox", { name: new RegExp(P.form.firstName) }), "Nora");
    await user.type(within(dialog).getByRole("textbox", { name: new RegExp(P.form.lastName) }), "Nová");
    await user.selectOptions(within(dialog).getByRole("combobox", { name: new RegExp(P.form.locale) }), "sk");
    await user.click(within(dialog).getByRole("button", { name: P.create }));

    await waitFor(() => expect(sent.filter((s) => s.method === "POST")).toHaveLength(1));
    expect(sent.find((s) => s.method === "POST")?.body).toEqual({
      email: "Nora@Example.org",
      firstName: "Nora",
      lastName: "Nová",
      locale: "sk",
    });
    expect(await screen.findByText(/sent nora@hel\.fi an e-mail/)).toBeInTheDocument();
  });

  it("deletes the ticked people in one Change after a confirm naming the count (T-5606)", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent) });

    await screen.findByRole("link", { name: "Jana Nováková" });
    expect(screen.getByText(P.bulk.hint)).toBeInTheDocument();
    expect(screen.queryByRole("toolbar")).toBeNull();
    await user.click(screen.getByRole("checkbox", { name: P.bulk.selectAll }));
    const toolbar = screen.getByRole("toolbar", { name: "2 selected" });
    await user.click(within(toolbar).getByRole("button", { name: P.bulk.delete }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete 2 people?" });
    expect(dialog).toHaveTextContent(/One change takes all of them out of every group/);
    await user.click(within(dialog).getByTestId("confirm-accept"));

    await waitFor(() => expect(sent.filter((s) => s.path.endsWith("/people/remove"))).toHaveLength(1));
    const [removal] = sent.filter((s) => s.path.endsWith("/people/remove"));
    expect(removal).toMatchObject({ method: "POST", body: { ids: ["jana-id", "eva-id"] } });
    expect(sent.filter((s) => s.method === "DELETE")).toHaveLength(0);
    expect(await screen.findByText(/chg-0000009/)).toBeInTheDocument();
    expect(screen.queryByRole("toolbar")).toBeNull();
    await expectNoViolations(screen.getByRole("table", { name: P.caption }).closest("section") as HTMLElement);
  });

  it("disables the ticked people and names whom the server refused (T-5606)", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { refuseEva: true }) });

    await user.click(await screen.findByRole("checkbox", { name: "Select Jana Nováková" }));
    await user.click(screen.getByRole("checkbox", { name: "Select Eva Kováčová" }));
    await user.click(screen.getByRole("button", { name: P.bulk.disable }));
    const dialog = await screen.findByRole("alertdialog", { name: "Disable 2 people?" });
    await user.click(within(dialog).getByTestId("confirm-accept"));

    const status = await screen.findByText(/1 of 2 disabled/);
    expect(status).toHaveTextContent("eva@hel.fi (eva@hel.fi holds a right you do not)");
    expect(sent.filter((s) => s.path.endsWith("/disable")).map((s) => s.path)).toEqual([
      "/api/v1/organization/people/jana-id/disable",
      "/api/v1/organization/people/eva-id/disable",
    ]);
  });

  it("offers a bulk delete only to a caller whose role deletes people, with the reason (T-5606)", async () => {
    const user = userEvent.setup();
    renderPage(<People />, { path: "/organization/people", answer: answer([], { verbs: ["read", "disable"] }) });

    await user.click(await screen.findByRole("checkbox", { name: "Select Jana Nováková" }));
    await waitFor(() =>
      expectDenied(screen.getByRole("button", { name: P.bulk.delete }), /^Only an organization administrator can /),
    );
    expect(screen.getByRole("button", { name: P.bulk.disable })).not.toHaveAttribute("aria-disabled");
  });

  it("shows a temporary password once, and it is gone after its dialog closes", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<People />, { path: "/organization/people", answer: answer(sent, { smtp: false }) });

    await user.click(await screen.findByRole("button", { name: P.new }));
    const form = await screen.findByRole("dialog", { name: P.newTitle });
    await user.type(within(form).getByRole("textbox", { name: new RegExp(P.form.email) }), "nora@example.org");
    await user.type(within(form).getByRole("textbox", { name: new RegExp(P.form.firstName) }), "Nora");
    await user.type(within(form).getByRole("textbox", { name: new RegExp(P.form.lastName) }), "Nová");
    await user.click(within(form).getByRole("button", { name: P.create }));

    const shown = await screen.findByRole("dialog", { name: P.password.title });
    expect(within(shown).getByRole("textbox", { name: P.password.label })).toHaveValue(PASSWORD);
    expect(within(shown).getByText(P.password.warning)).toBeInTheDocument();
    await expectNoViolations(shown);

    await user.click(within(shown).getByRole("button", { name: P.password.done }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: P.password.title })).toBeNull());
    expectPasswordGone(log);
  });
});

describe("a person's page", () => {
  let log: ReturnType<typeof vi.spyOn>[] = [];

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    log = (["log", "info", "warn", "error", "debug"] as const).map((level) => vi.spyOn(console, level));
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("names the groups, platform roles by scope and application roles, each linked", async () => {
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent) });

    expect(await screen.findByRole("heading", { name: "Jana Nováková", level: 1 })).toBeInTheDocument();
    const groups = (await screen.findByRole("heading", { name: P.groups })).closest("section") as HTMLElement;
    expect(within(groups).getByRole("link", { name: "stewards" })).toHaveAttribute("href", "/organization/groups/stewards");
    const platform = screen.getByRole("heading", { name: P.platformRoles }).closest("section") as HTMLElement;
    expect(platform).toHaveTextContent("viewer");
    expect(platform).toHaveTextContent("through the group stewards");
    expect(platform).toHaveTextContent("Project helsinki");
    const apps = screen.getByRole("heading", { name: P.appRoles }).closest("section") as HTMLElement;
    expect(within(apps).getByRole("link", { name: "alerts" })).toHaveAttribute("href", "/projects/helsinki/apps/alerts");
    for (const section of [groups, platform, apps]) await expectNoViolations(section);
  });

  it.each([
    ["disable", "/api/v1/organization/people/jana-id/disable", "POST"],
    ["sign-out", "/api/v1/organization/people/jana-id/sign-out", "POST"],
    ["remove-second-factor", "/api/v1/organization/people/jana-id/remove-second-factor", "POST"],
    ["reset-password", "/api/v1/organization/people/jana-id/reset-password", "POST"],
  ] as const)("asks before %s, sends nothing on Cancel, and reports what happened", async (action, path, method) => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent) });

    const button = await screen.findByRole("button", { name: P.action[action] });
    await user.click(button);
    let dialog = await screen.findByRole("alertdialog", { name: P.confirm[action].title.replace("{name}", "Jana Nováková") });
    // Cancel holds the focus: Enter on a dialog that arrived under the pointer costs nothing.
    await waitFor(() => expect(within(dialog).getByTestId("confirm-cancel")).toHaveFocus());
    await user.click(within(dialog).getByTestId("confirm-cancel"));
    expect(sent.filter((s) => s.method !== "GET")).toEqual([]);

    await user.click(button);
    dialog = await screen.findByRole("alertdialog", { name: P.confirm[action].title.replace("{name}", "Jana Nováková") });
    await user.click(within(dialog).getByTestId("confirm-accept"));
    await waitFor(() => expect(sent.filter((s) => s.method === method && s.path === path)).toHaveLength(1));
    expect(await screen.findByText(P.done[action])).toBeInTheDocument();
  });

  it("deletes through the Change it proposes", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent) });

    await user.click(await screen.findByRole("button", { name: P.action.delete }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete Jana Nováková?" });
    expect(dialog).toHaveTextContent(/a Change takes them out of each/);
    await user.click(within(dialog).getByTestId("confirm-accept"));
    await waitFor(() => expect(sent.filter((s) => s.method === "DELETE")).toHaveLength(1));
    expect(await screen.findByText(/chg-0000009/)).toBeInTheDocument();
  });

  it("hands a reset's temporary password once and drops it when the dialog closes", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent, { smtp: false }) });

    await user.click(await screen.findByRole("button", { name: P.action["reset-password"] }));
    await user.click(within(await screen.findByRole("alertdialog")).getByTestId("confirm-accept"));
    const shown = await screen.findByRole("dialog", { name: P.password.title });
    expect(within(shown).getByRole("textbox", { name: P.password.label })).toHaveValue(PASSWORD);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: P.password.title })).toBeNull());
    expectPasswordGone(log);
  });

  it("disables every action the caller's role does not allow, with the reason", async () => {
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent, { verbs: ["read"] }) });

    const reason = /^Only an organization administrator can /;
    await waitFor(() => expectDenied(screen.getByRole("button", { name: P.action.disable }), reason));
    for (const action of ["reset-password", "remove-second-factor", "sign-out", "delete"] as const) {
      expectDenied(screen.getByRole("button", { name: P.action[action] }), reason);
    }
    expectDenied(screen.getByRole("button", { name: P.edit }), reason);
  });

  it("edits only what changed", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    renderPage(<PersonPage id="jana-id" />, { path: "/organization/people/jana-id", answer: answer(sent) });

    // The guard refuses while the rights are read (T-5833): open once the answer is in.
    const edit = await screen.findByRole("button", { name: P.edit });
    await waitFor(() => expectOpen(edit));
    await user.click(edit);
    const dialog = await screen.findByRole("dialog", { name: "Edit Jana Nováková" });
    const last = within(dialog).getByRole("textbox", { name: new RegExp(P.form.lastName) });
    await user.clear(last);
    await user.type(last, "Horváthová");
    await user.click(within(dialog).getByRole("button", { name: P.save }));
    await waitFor(() => expect(sent.filter((s) => s.method === "PATCH")).toHaveLength(1));
    expect(sent.find((s) => s.method === "PATCH")?.body).toEqual({ lastName: "Horváthová" });
  });
});

describe("an edit's body", () => {
  it("keeps an address that differs only in case, so no new verification is asked", () => {
    const stored = { ...JANA, email: "Jana.Novakova@Hel.fi" } as Parameters<typeof changedFields>[0];
    const form = { email: "jana.novakova@hel.fi", firstName: "Jana", lastName: "Nováková", locale: "sk" };
    expect(changedFields(stored, form)).toEqual({});
    expect(changedFields(stored, { ...form, email: "jana@hel.fi" })).toEqual({ email: "jana@hel.fi" });
  });
});
