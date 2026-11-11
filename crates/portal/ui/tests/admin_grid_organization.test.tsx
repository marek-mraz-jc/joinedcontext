// covers (T-2137, the module gate in gate_modules.test.ts): src/components/grid/AdminGrid.tsx.
/**
 * T-6749: the Administration lists on the shared grid (Agents, Costs, Health, Data access,
 * Limits, Projects, Applications) and Project settings → Your access. Each page shows the rows
 * its read returned; the grid searches and sorts them, and a failed read is the API's reason.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { Agents } from "../src/pages/organization/Agents";
import { Costs } from "../src/pages/organization/Costs";
import { DataAccessMatrix } from "../src/pages/organization/DataAccess";
import { OrganizationLimitsView } from "../src/pages/organization/OrganizationLimitsView";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { ValidationHealth } from "../src/pages/organization/ValidationHealth";
import { ProjectSettingsPage } from "../src/pages/projectSettings/ProjectSettingsPage";
import { json, list, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const rowNames = () =>
  [...document.querySelectorAll(".jc-data-grid tbody tr")].map((tr) => tr.querySelector("th, td")?.textContent ?? "");
const search = (value: string) =>
  fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value } });
const sortBy = (label: string, item: string) => {
  fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${label}` }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: item }));
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  try {
    window.localStorage.clear();
  } catch {
    // no storage in this run
  }
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const agent = (name: string, owner: string) => ({
  kind: "AgentProfile",
  project: "helsinki",
  name,
  title: null,
  owner: { type: "person", name: owner },
  actsAs: "self",
  lastActivity: null,
  spend: { used: null, cap: null },
  state: "active",
  spaces: [],
  endpoints: [],
});

describe("Administration lists on the shared grid", () => {
  it("Agents: sorts by owner, searches, and shows the empty state and the API's reason", async () => {
    const t = en.organization.agents;
    let answer: () => Response = () => json({ items: [agent("bravo", "zoe@hel.fi"), agent("alpha", "ana@hel.fi")] });
    renderPage(<Agents />, {
      path: "/organization/agents",
      answer: (url) => (url.pathname === "/api/v1/organization/agents" ? answer() : undefined),
    });
    await screen.findByRole("link", { name: "alpha" });
    sortBy(t.column.owner, en.entityGrid.sortAsc);
    expect(rowNames()[0]).toContain("alpha");
    search("zoe");
    expect(rowNames()).toHaveLength(1);
    search("nobody");
    expect(screen.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.clearFilters }));
    expect(rowNames()).toHaveLength(2);
    cleanup();
    answer = () => json({ items: [] });
    renderPage(<Agents />, { path: "/organization/agents", answer: (url) => (url.pathname === "/api/v1/organization/agents" ? answer() : undefined) });
    expect(await screen.findByText(t.empty)).toBeInTheDocument();
    cleanup();
    answer = () => problem(503, "The agent index is not answering.");
    renderPage(<Agents />, { path: "/organization/agents", answer: (url) => (url.pathname === "/api/v1/organization/agents" ? answer() : undefined) });
    expect(await screen.findByText(/The agent index is not answering/)).toBeInTheDocument();
  });

  it("Costs: searches the projects, keeps the platform row, and shows the API's reason", async () => {
    const project = (name: string, total: number) => ({
      project: name, node: total, storage: 0, ai: 0, total, share: 0.5, spaces: [], assistants: [],
    });
    const body = {
      month: "2026-09", currency: "EUR", measuredAt: "2026-10-22T01:00:00Z",
      prices: { currency: "EUR", perUsd: 0.9, nodes: [], volumes: [], fixed: [] },
      projects: [project("doprava", 10), project("skola", 20)],
      platform: { node: 5, storage: 1, total: 6 }, infrastructure: 36, ai: 0, total: 36,
    };
    let reply: () => Response = () => json(body);
    renderPage(<Costs />, {
      path: "/organization/costs",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname === "/api/v1/organization/costs") return reply();
        return undefined;
      },
    });
    await screen.findByText("doprava");
    expect(rowNames()).toHaveLength(3);
    sortBy(en.organization.costs.total, en.entityGrid.sortDescNumber);
    expect(rowNames()[0]).toContain("skola");
    search("skola");
    expect(rowNames()).toHaveLength(1);
    cleanup();
    reply = () => problem(500, "The cost meter is down.");
    renderPage(<Costs />, {
      path: "/organization/costs",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname === "/api/v1/organization/costs") return reply();
        return undefined;
      },
    });
    expect(await screen.findByText(/The cost meter is down/)).toBeInTheDocument();
  });

  it("Health: searches the checks and shows the empty state", async () => {
    const check = (name: string, state: string) => ({ check: name, state });
    let checks = [check("authz", "unreadable"), check("sweep", "unreadable")];
    renderPage(<ValidationHealth />, {
      path: "/organization/health",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname === "/api/v1/organization/health") return json({ checks });
        return undefined;
      },
    });
    await screen.findByText("authz");
    search("swe");
    expect(rowNames()).toHaveLength(1);
    cleanup();
    checks = [];
    renderPage(<ValidationHealth />, {
      path: "/organization/health",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname === "/api/v1/organization/health") return json({ checks });
        return undefined;
      },
    });
    expect(await screen.findByText(en.organization.health.empty)).toBeInTheDocument();
  });

  it("Data access: one column per group, searched and sorted by the endpoint", async () => {
    const cell = (granted: boolean) => ({ granted, rules: [], throughOrganization: granted, unanswered: false });
    const row = (name: string, granted: boolean) => ({
      project: "helsinki", name, slug: `${name}zzzzzzzzzzzzzzzzzzzzzz`, title: null, audience: "organization",
      organization: "helsinki", cells: [cell(granted), cell(false)],
    });
    renderPage(<DataAccessMatrix />, {
      path: "/organization/data-access",
      answer: (url) =>
        url.pathname === "/api/v1/organization/data-access"
          ? json({ columns: [{ kind: "organization" }, { kind: "group", group: "analysts" }], rows: [row("vehicles", true), row("counts", false)], unanswered: 0 })
          : undefined,
    });
    await screen.findByRole("link", { name: "vehicles" });
    expect(screen.getByRole("columnheader", { name: new RegExp(en.dataAccess.everyone) })).toBeInTheDocument();
    sortBy(en.dataAccess.endpoint, en.entityGrid.sortAsc);
    expect(rowNames()[0]).toContain("counts");
    search("vehicles");
    expect(rowNames()).toHaveLength(1);
  });

  it("Limits: searches the per-project quota and says when no project is readable", async () => {
    const limits = (names: string[]) => ({
      entries: [],
      projects: names.map((project) => ({ project, origin: "project" as const, quota: { contextSpaces: { limit: 12, used: 3 } } })),
    });
    const view = (names: string[]) => (
      <OrganizationLimitsView limits={limits(names)} />
    );
    renderPage(view(["helsinki", "espoo"]), { path: "/organization/settings", answer: () => undefined });
    await screen.findByText("helsinki");
    const grid = screen.getByRole("table", { name: en.organization.limits.perProject });
    expect(within(grid).getAllByRole("row")).toHaveLength(3);
    cleanup();
    renderPage(view([]), { path: "/organization/settings", answer: () => undefined });
    expect(await screen.findByText(en.organization.limits.noProjects)).toBeInTheDocument();
  });

  it("Projects and Applications: search the rows the reads returned", async () => {
    const answer = (url: URL) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/projects") return json(list([{ name: "helsinki" }, { name: "espoo" }]));
      if (url.pathname.endsWith("/apps")) {
        const project = url.pathname.split("/")[4];
        return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "App", metadata: { name: `${project}-portal`, namespace: project }, spec: {} }]));
      }
      return undefined;
    };
    renderPage(<OrganizationPage tab="projects" anchor="helsinki" />, { path: "/organization/projects", answer });
    await screen.findByRole("link", { name: "helsinki" });
    search("esp");
    expect(rowNames()).toHaveLength(1);
    cleanup();
    renderPage(<OrganizationPage tab="applications" anchor="helsinki" />, { path: "/organization/applications", answer });
    await screen.findByRole("link", { name: "espoo-portal" });
    expect(rowNames()).toHaveLength(2);
    sortBy(en.organization.applications.name, en.entityGrid.sortDesc);
    expect(rowNames()[0]).toContain("helsinki-portal");
  });

  it("Your access: shows the grants, searched, and the empty sentence", async () => {
    const grant = (role: string, kinds: string[]) => ({ role, binding: "admins", scope: "project:helsinki", rule: { kinds, verbs: ["propose"] } });
    let grants = [grant("steward", ["Endpoint"]), grant("analyst", ["Pipeline"])];
    renderPage(<ProjectSettingsPage project="helsinki" tab="access" />, {
      path: "/projects/helsinki/settings/access",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json({ project: "helsinki", bootstrap: false, grants }) : undefined),
    });
    await screen.findByText("steward");
    search("pipelines");
    await waitFor(() => expect(rowNames()).toHaveLength(1));
    expect(rowNames()[0]).toContain("analyst");
    cleanup();
    grants = [];
    renderPage(<ProjectSettingsPage project="helsinki" tab="access" />, {
      path: "/projects/helsinki/settings/access",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json({ project: "helsinki", bootstrap: false, grants }) : undefined),
    });
    expect(await screen.findByText(en.projectSettings.access.none)).toBeInTheDocument();
  });
});
