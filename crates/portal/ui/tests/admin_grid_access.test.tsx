/**
 * T-6749: the access lists on the shared data grid (Groups, role bindings, effective permissions,
 * an account's access matrix and an agent profile's operations), each walked through its real
 * page: a sort from the header menu, the search box, and an empty state or a failed read.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { AccountAccess } from "../src/pages/access/AccountAccess";
import { answeringChecks } from "./checks";

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
type Answer = (path: string) => unknown | undefined;

function stub(answer: Answer, failing?: string) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = typeof input === "string" || input instanceof URL ? null : input;
      const path = new URL(request ? request.url : String(input), window.location.origin).pathname;
      const json = (body: unknown, status = 200, type = "application/json") =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } });
      if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "jana", roles: [] });
      if (failing && path.endsWith(failing)) {
        return json({ type: "about:blank", title: "Unavailable", status: 503, detail: "The store did not answer." }, 503, "application/problem+json");
      }
      return json(answer(path) ?? list([]));
    }),
  );
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
}

function renderApp(path: string) {
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The first cell of every row of the grid: the row's name. */
const names = (caption?: string) =>
  [...(caption ? [screen.getByRole("table", { name: caption })] : [document.body]).flatMap((root) => [...root.querySelectorAll("tbody tr")])].map((tr) => tr.querySelector("th, td")?.textContent ?? "");

function sortBy(label: string, option: string) {
  fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${label}` }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: option }));
}

function search(value: string, caption?: string) {
  const grid = caption ? (screen.getByRole("table", { name: caption }).closest(".jc-data-grid") as HTMLElement) : undefined;
  const box = grid ? within(grid).getByRole("searchbox", { name: en.dataGrid.searchRows }) : screen.getByRole("searchbox", { name: en.dataGrid.searchRows });
  fireEvent.change(box, { target: { value } });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  try {
    window.localStorage.clear();
  } catch {
    // no storage in this run
  }
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const READ_ALL = (kinds: string[]) => ({
  project: "banskabystrica",
  bootstrap: false,
  grants: [
    { role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] } },
    { role: "reader", binding: "r", scope: "organization", rule: { kinds, verbs: ["read", "propose"] } },
  ],
});

describe("Groups on the grid", () => {
  const group = (name: string, user: string) => ({
    apiVersion: API,
    kind: "Group",
    metadata: { name, namespace: "org" },
    spec: { members: [{ user }] },
  });
  const answer: Answer = (path) => {
    if (path.endsWith("/permissions/me")) return READ_ALL(["Group", "RoleBinding"]);
    if (path === "/api/v1/projects/org/groups") return list([group("city-leads", "lead@hel.fi"), group("air-viewers", "air@hel.fi")]);
    if (path === "/api/v1/projects") return list([{ name: "banskabystrica" }]);
    return undefined;
  };

  it("sorts from the header menu and searches the rows", async () => {
    stub(answer);
    renderApp("/organization/groups");
    await screen.findByRole("link", { name: "city-leads" });
    sortBy(en.access.groups.group, en.entityGrid.sortAsc);
    expect(names().map((text) => text.trim())).toEqual(["air-viewers", "city-leads"]);
    search("zzz");
    expect(screen.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.clearFilters }));
    expect(names()).toHaveLength(2);
  });

  it("says what a group is for when there are none", async () => {
    stub((path) => (path.endsWith("/permissions/me") ? READ_ALL(["Group", "RoleBinding"]) : undefined));
    renderApp("/organization/groups");
    expect(await screen.findByText(en.access.groups.empty)).toBeInTheDocument();
  });

  it("shows the API's reason when the read fails", async () => {
    stub(answer, "/api/v1/projects/org/groups");
    renderApp("/organization/groups");
    expect(await screen.findByText(/The store did not answer/)).toBeInTheDocument();
    expect(screen.queryByText(en.access.groups.empty)).toBeNull();
  });
});

describe("Role bindings on the grid", () => {
  const binding = (name: string, user: string, role: string) => ({
    apiVersion: API,
    kind: "RoleBinding",
    metadata: { name, namespace: "org" },
    spec: { subjects: [{ user }], role, scope: { project: "helsinki" } },
  });
  const answer: Answer = (path) => {
    if (path.endsWith("/permissions/me")) return READ_ALL(["RoleBinding", "Role", "ContextSpace"]);
    if (path === "/api/v1/projects/org/rolebindings") {
      return list([binding("a", "ann@hel.fi", "steward"), binding("b", "bob@hel.fi", "org-admin")]);
    }
    return undefined;
  };

  it("sorts by role and searches the rows", async () => {
    stub(answer);
    renderApp("/projects/helsinki/settings/members");
    await screen.findAllByText("ann@hel.fi");
    sortBy(en.access.roles.role, en.entityGrid.sortAsc);
    expect(names().map((text) => text.trim())).toEqual(["bob@hel.fi", "ann@hel.fi"]);
    search("steward");
    expect(names()).toHaveLength(1);
  });

  it("offers the first grant when nobody holds a role", async () => {
    stub((path) => (path.endsWith("/permissions/me") ? READ_ALL(["RoleBinding", "Role", "ContextSpace"]) : undefined));
    renderApp("/projects/helsinki/settings/members");
    expect(await screen.findByText(en.access.roles.empty)).toBeInTheDocument();
  });

  it("shows the API's reason when the read fails", async () => {
    stub(answer, "/api/v1/projects/org/rolebindings");
    renderApp("/projects/helsinki/settings/members");
    expect(await screen.findByText(/The store did not answer/)).toBeInTheDocument();
  });
});

describe("Effective permissions on the grid", () => {
  const grant = (type: string, actions: string[]) => ({ resource: { type }, actions, attributes: "*", constraints: {} });
  const answer: Answer = (path) => {
    if (path.endsWith("/access")) {
      return {
        subject: { type: "user", id: "janko" },
        resource: { type: "endpoint", id: "slug", space: "air" },
        permissions: [grant("Station", ["retrieveEntity"]), grant("Air", ["retrieveEntity", "updateAttrs"])],
        prohibitions: [],
      };
    }
    if (path.endsWith("/endpoints")) {
      return list([{ apiVersion: API, kind: "Endpoint", metadata: { name: "e", namespace: "helsinki" }, spec: { slug: "slug", audience: "public" } }]);
    }
    return undefined;
  };

  it("sorts the grants by type and searches them", async () => {
    stub(answer);
    renderApp("/projects/helsinki/settings/access");
    await screen.findAllByRole("rowheader", { name: /Station/ });
    sortBy(en.access.matrix.type, en.entityGrid.sortAsc);
    expect(names(en.access.matrix.title).map((text) => text.trim())).toEqual(["Air", "Station"]);
    search("Station", en.access.matrix.title);
    expect(names(en.access.matrix.title)).toHaveLength(1);
  });

  it("shows nothing as a grid when the endpoint grants nothing", async () => {
    stub((path) => (path.endsWith("/access") ? { subject: { id: "janko" }, resource: { space: "air" }, permissions: [], prohibitions: [] } : answer(path)));
    renderApp("/projects/helsinki/settings/access");
    expect(await screen.findByText(en.access.matrix.empty)).toBeInTheDocument();
  });
});

describe("An account's access matrix on the grid", () => {
  const grant = (index: number, role: string, kinds: string[]) => ({
    index,
    role,
    scope: "project:helsinki",
    state: "active",
    rules: [{ kinds, verbs: ["propose"] }],
  });

  function show(grants: unknown[], failing?: string) {
    stub(
      (path) =>
        path.endsWith("/access")
          ? { account: "vendor-ci", project: "helsinki", delegates: false, mayPropose: true, mayRevoke: false, grants }
          : undefined,
      failing,
    );
    const root = createRootRoute({ component: Outlet });
    const page = createRoute({
      getParentRoute: () => root,
      path: "/",
      component: () => <AccountAccess project="helsinki" account="vendor-ci" />,
    });
    window.history.pushState({}, "", "/");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <RouterProvider router={createRouter({ routeTree: root.addChildren([page]) })} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  it("sorts the kinds and searches the cells", async () => {
    show([grant(0, "editor", ["Pipeline", "Endpoint"])]);
    await screen.findByRole("table", { name: "What vendor-ci may do, by kind" });
    sortBy(en.access.agent.kind, en.entityGrid.sortDesc);
    expect(names().map((text) => text.trim())).toEqual(["Pipeline", "Endpoint"]);
    search("Endpoint");
    expect(names()).toHaveLength(1);
  });

  it("says the gateway is the only way in when no grant names a kind", async () => {
    show([]);
    expect(await screen.findByText(en.access.agent.gatewayOnly)).toBeInTheDocument();
  });

  it("shows the API's reason when the read fails", async () => {
    show([], "/access");
    expect(await screen.findByText(/The store did not answer/)).toBeInTheDocument();
  });
});

describe("An agent profile's operations on the grid", () => {
  const ACCESS = {
    items: [
      {
        name: "app-builder",
        title: "Application builder",
        role: "builder",
        access: { operations: ["b_op"] },
        egressHosts: [],
        operations: [
          { name: "b_op", readOnly: true, profile: true, person: true, reason: null },
          { name: "a_op", readOnly: false, profile: true, person: false, reason: "no role grants propose" },
        ],
      },
    ],
  };

  it("sorts the operations by name and searches the reasons", async () => {
    stub((path) => (path.endsWith("/assistant/access") ? ACCESS : undefined));
    vi.stubGlobal("EventSource", class { addEventListener() {} removeEventListener() {} close() {} });
    renderApp("/projects/helsinki/assistant");
    await screen.findByRole("table", { name: en.assistantPage.access.caption.replace("{name}", "app-builder"), hidden: true });
    sortBy(en.assistantPage.access.operation, en.entityGrid.sortAsc);
    expect(names().filter((text) => text.endsWith("_op"))).toEqual(["a_op", "b_op"]);
    search("no role");
    await waitFor(() => expect(names().filter((text) => text.endsWith("_op"))).toEqual(["a_op"]));
  });

  it("names an empty profile and shows the reason a failed read gave", async () => {
    stub((path) => (path.endsWith("/assistant/access") ? { items: [{ ...ACCESS.items[0], operations: [] }] } : undefined));
    vi.stubGlobal("EventSource", class { addEventListener() {} removeEventListener() {} close() {} });
    renderApp("/projects/helsinki/assistant");
    expect(await screen.findByText(en.assistantPage.access.noOperations, undefined, { timeout: 4000 })).toBeInTheDocument();
  });
});
