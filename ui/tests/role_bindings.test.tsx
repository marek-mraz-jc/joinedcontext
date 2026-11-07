/**
 * Project → Access → People and roles (T-0740, AG-77, PF-52): who holds which role here, a grant
 * proposed from a form the assistant may have filled, a refusal shown in the API's words, and the
 * removal of a binding with its name typed back, all against the organization's routes.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/access/RoleBindings.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { rememberPrefill } from "../src/assistant/state";
import { answeringChecks, checksSoFar, expectDenied } from "./checks";
import { findFormPage, queryFormPage } from "./formPage";

/**
 * The verb the remove control is announced with, read from the bundle rather than typed here.
 *
 * T-1425 made one verb of it in all four locales and the two cases below still looked for
 * "Delete", so they failed on a rename that was the point of that task. The accessible name is
 * `resourceDelete.action` with the resource's name filled in, so the verb is what stands before
 * the placeholder (UI-23).
 */
const REMOVE = new RegExp("^" + en.resourceDelete.action.split("{name}")[0].trim() + "\\b");


const PROJECT = "helsinki";

const binding = (name: string, subject: Record<string, string>, role: string, scope: Record<string, string>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "RoleBinding",
  metadata: { name, namespace: "org" },
  spec: { subjects: [subject], role, scope },
});

const BINDINGS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    binding("admins", { user: "someone@hel.fi" }, "org-admin", { organization: "hel" }),
    binding("stewards", { user: "demo.steward@hel.fi" }, "steward", { project: PROJECT }),
    binding("bikes-team", { group: "bikes" }, "pipeline-editor", { contextSpace: "citybikes" }),
    binding("espoo-stewards", { user: "someone@espoo.fi" }, "steward", { project: "espoo" }),
  ],
};

const ROLES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: ["org-admin", "pipeline-editor", "steward"].map((name) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Role",
    metadata: { name, namespace: "org" },
    spec: { rules: [] },
  })),
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "citybikes", namespace: PROJECT }, spec: {} },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-00000301", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

const ADMIN = [
  { rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } },
  { rule: { kinds: ["Role", "ContextSpace"], verbs: ["read"] } },
];
/** Reads who holds what and changes nothing: the lists are asked for, every change is refused. */
const READER = [{ rule: { kinds: ["RoleBinding", "Role", "ContextSpace"], verbs: ["read"] } }];

function renderAccess(options: { grants: unknown[]; refusal?: string }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } }));
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "demo.steward", email: "demo.steward@hel.fi", roles: [] });
    }
    if (path.endsWith("/permissions/me")) {
      return json({ grants: options.grants });
    }
    if (request.method === "POST" || request.method === "DELETE") {
      return options.refusal
        ? json(
            { type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: options.refusal },
            403,
            "application/problem+json",
          )
        : json(CHANGE, 202);
    }
    if (path === "/api/v1/projects/org/rolebindings") {
      return json(BINDINGS);
    }
    if (path === "/api/v1/projects/org/roles") {
      return json(ROLES);
    }
    if (path === `/api/v1/projects/${PROJECT}/spaces`) {
      return json(SPACES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

const writes = (fetchMock: ReturnType<typeof renderAccess>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method !== "GET");

const roles = () => screen.getByRole("table", { name: en.access.roles.caption.replace("{project}", PROJECT) });

describe("people and roles", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    window.history.pushState({}, "", `/projects/${PROJECT}/settings/members`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    window.history.pushState({}, "", "/");
  });

  // The organization's grants are on Organization → Members (Architecture/09 §14.3, T-2606).
  it("lists the roles held over this project and its spaces, not the organization's or another project's", async () => {
    renderAccess({ grants: ADMIN });

    expect(await screen.findByText("demo.steward@hel.fi")).toBeInTheDocument();
    const table = roles();
    expect(within(table).getByText(en.access.roles.project.replace("{name}", PROJECT))).toBeInTheDocument();
    expect(within(table).queryByText("someone@hel.fi")).not.toBeInTheDocument();
    expect(within(table).getByText("Group bikes")).toBeInTheDocument();
    expect(within(table).getByText("Context space citybikes")).toBeInTheDocument();
    expect(within(table).queryByText("someone@espoo.fi")).not.toBeInTheDocument();
    expect(within(table).getAllByRole("button", { name: /^Edit\b/ })).toHaveLength(2);
    expect(within(table).getAllByRole("button", { name: REMOVE })).toHaveLength(2);
  });

  it("keeps every change a viewer may not make where it is, disabled with the reason", async () => {
    renderAccess({ grants: READER });

    expect(await screen.findByText("demo.steward@hel.fi")).toBeInTheDocument();
    expectDenied(screen.getByRole("button", { name: en.access.roles.grant }), /Only an organization administrator can add or change role assignments/);
    for (const button of within(roles()).queryAllByRole("button", { name: /^(Edit|Remove)\b/ })) {
      expectDenied(button);
    }
  });

  it("asks a role that does not read the bindings for none of them and says who can grant it", async () => {
    // demo.viewer on the walk (T-3852): the 404 of the unasked-for list was a red failure box.
    const fetchMock = renderAccess({ grants: [] });

    expect(await screen.findByText(en.access.roles.hidden)).toBeInTheDocument();
    const asked = fetchMock.mock.calls.map(([input]) => new URL((input as Request).url).pathname);
    expect(asked.filter((path) => /\/(rolebindings|roles|spaces)$/.test(path))).toEqual([]);
  });

  it("lists the bindings to a role that reads them but not the spaces, never stuck at loading", async () => {
    // T-4027: the unasked-for spaces list stayed pending, so the table never left "Loading".
    const fetchMock = renderAccess({ grants: [{ rule: { kinds: ["RoleBinding", "Role"], verbs: ["read"] } }] });

    expect(await screen.findByText("demo.steward@hel.fi")).toBeInTheDocument();
    expect(within(roles()).queryByText(en.app.loading)).not.toBeInTheDocument();
    const asked = fetchMock.mock.calls.map(([input]) => new URL((input as Request).url).pathname);
    expect(asked.filter((path) => path.endsWith("/spaces"))).toEqual([]);
  });

  it("proposes the binding the form shows to the organization's route and shows the red change", async () => {
    const fetchMock = renderAccess({ grants: ADMIN });

    await userEvent.click(await screen.findByRole("button", { name: en.access.roles.grant }));
    const dialog = await findFormPage(en.access.roles.grantTitle);
    const propose = within(dialog).getByRole("button", { name: en.access.roles.propose });
    // Propose is operable on an empty form and says which field is missing instead of being a
    // button that does nothing; it proposes only once the form is filled (T-1759, T-1492).
    await userEvent.click(propose);
    expect(writes(fetchMock), "an empty form proposed something").toHaveLength(0);
    expect(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel))).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    await userEvent.type(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel)), "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e{Enter}");
    await waitFor(() => expect(within(dialog).getByRole("option", { name: "steward" })).toBeInTheDocument());
    await userEvent.selectOptions(within(dialog).getByLabelText(new RegExp(en.access.roles.roleLabel)), "steward");
    await userEvent.selectOptions(within(dialog).getByLabelText(en.access.roles.whereLabel), "space:citybikes");
    await userEvent.click(propose);

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [post] = writes(fetchMock);
    expect(new URL(post.url).pathname).toBe("/api/v1/projects/org/rolebindings");
    await expect(post.clone().json()).resolves.toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "RoleBinding",
      metadata: { name: "2f1c6b1e-steward-citybikes", namespace: "org" },
      spec: {
        subjects: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }],
        role: "steward",
        scope: { contextSpace: "citybikes" },
      },
    });
    // Saved, the form goes back to the list and the change is shown there (T-2474).
    expect(await screen.findByText("chg-00000301")).toBeInTheDocument();
    expect(queryFormPage()).toBeNull();
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("POST /api/v1/projects/org/rolebindings"))).toBe(true);
  });

  it("opens the grant the assistant drafted, filled in, and shows a refusal in the API's words", async () => {
    const refusal = "a binding may not grant more than its proposer holds: missing delete on Endpoint (PF-52)";
    window.history.pushState({}, "", `/projects/${PROJECT}/settings/members?grant=jana-kovacova-org-admin-helsinki`);
    rememberPrefill(`/projects/${PROJECT}/settings/members?grant=jana-kovacova-org-admin-helsinki`, {
      ...binding("jana-kovacova-org-admin-helsinki", { user: "jana.kovacova" }, "org-admin", { project: PROJECT }),
    });
    renderAccess({ grants: ADMIN, refusal });

    const dialog = await findFormPage(en.access.roles.grantTitle);
    expect(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel))).toHaveValue("jana.kovacova");
    await waitFor(() => expect(within(dialog).getByLabelText(new RegExp(en.access.roles.roleLabel))).toHaveValue("org-admin"));
    expect(within(dialog).getByLabelText(en.access.roles.whereLabel)).toHaveValue("project");

    await userEvent.click(within(dialog).getByRole("button", { name: en.access.roles.propose }));
    // The API's words, without the requirement it cites for engineers (T-2756).
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(refusal.replace(" (PF-52)", ""));
    expect(alert).not.toHaveTextContent("PF-52");
  });

  it("opens the grant the assistant drafts while the person is already on the access page", async () => {
    renderAccess({ grants: ADMIN });
    await screen.findByText("demo.steward@hel.fi");
    expect(queryFormPage()).not.toBeInTheDocument();

    // What the dock does with a navigate event: the form in hand, then the route with the draft.
    const route = `/projects/${PROJECT}/settings/members?grant=jana-kovacova-steward-helsinki`;
    rememberPrefill(route, binding("jana-kovacova-steward-helsinki", { user: "jana.kovacova" }, "steward", { project: PROJECT }));
    act(() => {
      window.history.pushState({}, "", `${route}&draft=jana-kovacova-steward-helsinki`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    });

    const dialog = await findFormPage(en.access.roles.grantTitle);
    expect(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel))).toHaveValue("jana.kovacova");
    await waitFor(() => expect(within(dialog).getByLabelText(new RegExp(en.access.roles.roleLabel))).toHaveValue("steward"));
  });

  it("removes a binding only after its name is typed back, through the organization's route", async () => {
    const fetchMock = renderAccess({ grants: ADMIN });

    await screen.findByText("demo.steward@hel.fi");
    const [remove] = within(roles()).getAllByRole("button", { name: REMOVE });
    await userEvent.click(remove);
    const dialog = await screen.findByRole("dialog");
    const confirm = within(dialog).getByRole("button", { name: en.resourceDelete.propose });
    await userEvent.type(within(dialog).getByRole("textbox"), "steward");
    // `aria-disabled`, not `disabled`: a refusal that carries a reason keeps the button in the
    // tab order so the reason can be reached and read (T-1830, T-1743, UI-44). `expectDenied` is
    // that property in one call — the attribute, the tab order, and the reason it names.
    expectDenied(confirm, en.resourceDelete.needName.replace("{name}", "stewards"));
    await userEvent.type(within(dialog).getByRole("textbox"), "s");
    await userEvent.click(confirm);

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    expect(new URL(writes(fetchMock)[0].url).pathname).toBe("/api/v1/projects/org/rolebindings/stewards");
  });
});
