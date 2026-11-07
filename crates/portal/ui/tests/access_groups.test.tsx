/**
 * The groups of the organization on the Access page (T-0867, PF-62, PF-63, UI-44): who is in
 * each one, what the last reconcile had to correct in Keycloak, and a new group proposed as a
 * change — with the control disabled and the reason readable when the caller may not propose.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/access/Groups.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import en from "../src/locales/en.json";
import { answeringChecks, checksSoFar, expectDenied } from "./checks";
import { findFormPage } from "./formPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: [],
};

const LIST = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: LIST, kind: "List", items });

const GROUPS = list([
  {
    apiVersion: LIST,
    kind: "Group",
    metadata: { name: "city-leads", namespace: "org" },
    spec: {
      description: "The people who lead the city's projects",
      members: [{ user: "lead@hel.fi" }, { user: "newcomer@hel.fi" }],
    },
    status: {
      phase: "Live",
      conditions: [
        {
          type: "GroupSynced",
          status: "True",
          reason: "DriftCorrected",
          message: "sneaky@hel.fi was in the realm's group and in no manifest; the membership was removed",
        },
      ],
    },
  },
]);

/** What the caller may do; `verbs` is what their one role grants on `Group`. */
function renderAccess(verbs: string[]) {
  const posted: { path: string }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const method = request?.method ?? init?.method ?? "GET";
    const json = (body: unknown, status = 200) =>
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me")) {
      return json({
        project: "banskabystrica",
        bootstrap: false,
        grants: [
          {
            role: "org-admin",
            binding: "admins",
            scope: "organization",
            rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] },
          },
          {
            role: "group-keeper",
            binding: "keepers",
            scope: "organization",
            rule: { kinds: ["Group"], verbs },
          },
        ],
      });
    }
    if (method === "POST" && path === "/api/v1/projects/org/groups") {
      posted.push({ path });
      return json(
        {
          apiVersion: LIST,
          kind: "Change",
          metadata: { name: "chg-0000003b", namespace: "org" },
          status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
        },
        202,
      );
    }
    if (path === "/api/v1/projects/org/groups") {
      return json(GROUPS);
    }
    // The Organization page stands in the shell of the first project (T-2605).
    if (path === "/api/v1/projects") {
      return json(list([{ name: "banskabystrica" }]));
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
  return { posted };
}

describe("the groups of the organization on Organization → Groups", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/organization/groups");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the members and what the reconciler had to correct in Keycloak", async () => {
    renderAccess(["read", "propose"]);

    const row = (await screen.findByText("city-leads")).closest("tr") as HTMLElement;
    expect(within(row).getByText("lead@hel.fi")).toBeTruthy();
    expect(within(row).getByText("newcomer@hel.fi")).toBeTruthy();
    expect(within(row).getByText("Corrected")).toBeTruthy();
    expect(within(row).getByText(/sneaky@hel.fi/)).toBeTruthy();
  });

  it("proposes a new group into the organization as one change", async () => {
    const { posted } = renderAccess(["read", "propose"]);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "New group" }));
    const dialog = await findFormPage(/New group/);

    // T-2400 made this a form. The manifest is built from the fields, so the namespace is no
    // longer read out of a skeleton — it is what the POST goes to, asserted below. An untouched
    // form is refused at the field now rather than on the button, so what stays here is the
    // property: nothing is posted until the form is filled.
    const proposeButton = within(dialog).getByRole("button", { name: "Propose change" });
    await user.click(proposeButton);
    expect(posted, "an empty form does not post").toHaveLength(0);

    await user.type(
      within(dialog).getByLabelText(new RegExp(en.access.groups.field.name)),
      "park-wardens",
    );
    await user.click(within(dialog).getByRole("button", { name: "Add" }));
    await user.type(
      within(dialog).getByLabelText(new RegExp(en.access.groups.field.memberUser)),
      "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e",
    );
    // The verdict gate: a manifest is checked before it may be proposed (PF-57, T-0956), and the
    // form asks for that check by hand where the YAML view used to run it on the way out.
    await user.click(within(dialog).getByRole("button", { name: "Check" }));
    await user.click(proposeButton);

    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0].path).toBe("/api/v1/projects/org/groups");
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("POST /api/v1/projects/org/groups"))).toBe(true);
  });

  it("disables the control with the reason when the caller may only read groups", async () => {
    renderAccess(["read"]);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "New group" })).toHaveAttribute("aria-disabled");
    });
    expectDenied(
      screen.getByRole("button", { name: "New group" }),
      "Only an organization administrator can add or change groups.",
    );
  });
});
