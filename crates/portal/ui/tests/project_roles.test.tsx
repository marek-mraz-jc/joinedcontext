// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/access/Roles.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { narrowsAndOrders } from "./grid_checks";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { answeringChecks, checksSoFar, expectDenied } from "./checks";
import en from "../src/locales/en.json";
import { findFormPage } from "./formPage";
import { digestOf } from "../src/api/drafts";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-steward"],
};

const role = (name: string, namespace: string, kinds: string[], verbs: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Role",
  metadata: { name, namespace },
  spec: { rules: [{ kinds, verbs }] },
});

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

/** What the caller may do here, as `permissions/me` answers it (PF-50). */
const PERMISSIONS = {
  project: "banskabystrica",
  bootstrap: false,
  grants: [
    {
      role: "steward",
      binding: "jana-steward",
      rule: { kinds: ["Role", "Pipeline"], verbs: ["propose", "approve"] },
    },
  ],
};

function renderAccess(roles: unknown[] = [role("air-analyst", "banskabystrica", ["DataSource"], ["propose"])]) {
  const posted: { path: string; body: string }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const method = request?.method ?? init?.method ?? "GET";
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me")) {
      return json(PERMISSIONS);
    }
    if (method === "POST" && path === "/api/v1/projects/banskabystrica/roles") {
      // The text, not the stream: a `Request`'s body is read once, and a test that asserts what
      // was proposed has to be able to read it (T-2416).
      posted.push({ path, body: request ? await request.clone().text() : String(init?.body ?? "") });
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-0000002a", namespace: "banskabystrica" },
          status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
        },
        202,
      );
    }
    if (path === "/api/v1/projects/banskabystrica/drafts/Role/analyst") {
      const manifest = role("analyst", "banskabystrica", ["Pipeline"], ["propose"]);
      return json({
        project: "banskabystrica",
        kind: "Role",
        name: "analyst",
        manifest,
        verdict: { ok: true, findings: [], digest: digestOf(manifest), checkedAt: "2026-11-02T22:00:00Z" },
        touchedBy: "jana.kovacova",
        touchedKind: "assistant",
        version: 1,
        updatedAt: "2026-11-02T22:00:00Z",
      });
    }
    if (path === "/api/v1/projects/org/roles") {
      return json(list([role("org-admin", "org", ["Endpoint", "Role"], ["propose", "approve", "delete"])]));
    }
    if (path === "/api/v1/projects/banskabystrica/roles") {
      return json(list(roles));
    }
    return json(list([]));
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
  return { fetchMock, posted };
}

/** This project's own collection; `org`'s sits beside it in the same table. */
const ROLES_HERE = "/api/v1/projects/banskabystrica/roles";

/**
 * The smallest role this author may write: a name, and the one rule their own grant covers.
 * The kinds and the verbs are `<select multiple>`, which is what the schema's `uniqueItems`
 * enum renders as.
 */
async function fillTheRole(
  dialog: HTMLElement,
  user: ReturnType<typeof userEvent.setup>,
): Promise<void> {
  await user.type(
    within(dialog).getByLabelText(new RegExp(en.access.projectRoles.field.name)),
    "air-steward",
  );
  // Kinds and verbs are checkboxes, ticked one by one (T-4213).
  await user.click(within(dialog).getByRole("checkbox", { name: "Pipeline" }));
  await user.click(within(dialog).getByRole("checkbox", { name: en.choice.verb.propose }));
}

describe("the roles of a project on the Access page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/settings/roles");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // The organization's roles are on Organization → Roles (Architecture/09 §14.3, T-2606).
  it("lists the project's own roles, and not the organization's", async () => {
    renderAccess();

    const own = (await screen.findByText("air-analyst")).closest("tr") as HTMLElement;
    expect(within(own).getByText("Project banskabystrica")).toBeTruthy();
    expect(own.textContent).toMatch(/Propose: data sources/);
    expect(screen.queryByText("org-admin")).toBeNull();
  });

  // T-6206: the roles are on the shared grid: search, sort by rule count, filter by where.
  // T-6489, walk S-28: the assistant's draft opens the form, for the person to read and propose.
  it("opens the form on the role the assistant drafted", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/settings/roles?draft=analyst");
    renderAccess();

    const form = await findFormPage(en.access.projectRoles.newTitle);
    const name = within(form).getByLabelText(new RegExp(en.access.projectRoles.field.name));
    await waitFor(() => expect((name as HTMLInputElement).value).toBe("analyst"));
    expect((within(form).getByRole("checkbox", { name: en.choice.verb.propose }) as HTMLInputElement).checked).toBe(true);
  });

  it("searches, sorts by rule count and filters the roles", async () => {
    const two = { ...role("air-steward", "banskabystrica", ["Pipeline"], ["propose"]), spec: { rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }, { kinds: ["DataSource"], verbs: ["read"] }] } };
    renderAccess([two, role("air-analyst", "banskabystrica", ["DataSource"], ["propose"])]);
    const table = await screen.findByRole("table", { name: en.access.projectRoles.caption.replace("{project}", "banskabystrica") });
    await within(table).findByText("air-steward");
    await narrowsAndOrders(table, {
      search: { label: `Search ${en.access.projectRoles.title}`, text: "steward", kept: ["air-steward"] },
      sort: { column: en.access.projectRoles.ruleCount, ascending: ["air-analyst", "air-steward"] },
      filter: { value: /^Project banskabystrica/, kept: ["air-steward", "air-analyst"] },
    });
  });

  it("opens this project's role form and posts nothing until it is valid", async () => {
    // Until T-2400 this case typed a YAML skeleton. A Role is authored through its fields now,
    // and the fields themselves are held by `access_forms.test.tsx` (19 cases, including the
    // rules a person may not grant). What this file holds at App level is the journey: the door,
    // the check, and where the proposal goes.
    const { posted } = renderAccess();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await findFormPage(/New role/);
    expect(
      within(dialog).getByText(/banskabystrica/),
      "the form says which project the role belongs to",
    ).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: en.access.projectRoles.propose }));
    expect(posted, "an incomplete form does not post").toHaveLength(0);
  });

  it("proposes the filled role to this project's own collection, checked first", async () => {
    // The whole journey, which is what was lost when the YAML skeleton went: open, fill, Check,
    // Propose — and the proposal lands on `/api/v1/projects/banskabystrica/roles`, never on the
    // organization's collection beside it in the same table (PF-68, T-2416).
    const { posted } = renderAccess();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await findFormPage(/New role/);
    await fillTheRole(dialog, user);

    await user.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expect(checksSoFar(), "the kind is checked before it is proposed (PF-57)").toContain(
        `POST ${ROLES_HERE}`,
      ),
    );
    expect(posted, "checking is not proposing").toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: en.access.projectRoles.propose }));
    await waitFor(() => expect(posted).toHaveLength(1));

    expect(posted[0].path).toBe(ROLES_HERE);
    expect(JSON.parse(posted[0].body)).toMatchObject({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Role",
      metadata: { name: "air-steward", namespace: "banskabystrica" },
      spec: { rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }] },
    });
    // The change it answered with is what the person is shown, not a silent close.
    expect(await screen.findByText(/chg-0000002a/)).toBeTruthy();
  });

  it("refuses a proposal that was never checked, and says why", async () => {
    // PF-57 in strict mode (T-0956): the YAML view ran the check on the way out, the form asks
    // for it. A filled role with no verdict is refused at the button — reachable, with the
    // reason — and nothing reaches the collection.
    const { posted } = renderAccess();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await findFormPage(/New role/);
    await fillTheRole(dialog, user);

    const propose = within(dialog).getByRole("button", { name: en.access.projectRoles.propose });
    expectDenied(propose, en.drafts.proposeReason.none);
    await user.click(propose);
    expect(checksSoFar()).toEqual([]);
    expect(posted, "a role nobody checked is not proposed").toHaveLength(0);
  });
});
