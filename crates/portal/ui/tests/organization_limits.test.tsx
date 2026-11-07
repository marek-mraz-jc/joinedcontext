/**
 * T-2715 (PF-99, PF-101, PF-102; API/01 §28): Organization settings shows every policy and limit
 * in its catalog section, with the value in force, the range allowed and a one-line hint, and the
 * quota of each project; the form holds each number to the operator's bound, so a value above the
 * ceiling is refused before anything is proposed.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import validator from "../src/components/forms/validator";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { OrganizationLimitsView } from "../src/pages/organization/OrganizationLimitsView";
import { SECTIONS } from "../src/pages/organization/limits";
import type { LimitEntry, OrganizationLimits } from "../src/pages/organization/limits";
import { organizationSchema } from "../src/schemas/kinds";
import { expectNoAxeViolations, json, list, renderPage } from "./page_contract";

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "hel", namespace: "org" },
  spec: {
    domain: "hel.fi",
    locales: ["en"],
    defaultLocale: "en",
    projects: { quota: { contextSpaces: 10 } },
    policies: { apps: { public: "refused" } },
    limits: { edge: { requestsPerMinute: { web: 300 } } },
  },
};

const entry = (path: string, section: string, extra: Partial<LimitEntry>): LimitEntry => ({
  path,
  section,
  security: false,
  default: null,
  min: 0,
  max: null,
  value: null,
  origin: "default",
  ...extra,
});

const LIMITS: OrganizationLimits = {
  entries: [
    entry("spec.projects.quota.contextSpaces", "projects", { max: 20, value: 10, origin: "organization" }),
    entry("spec.limits.edge.requestsPerMinute.web", "edge", { default: 600, min: 10, max: 6000, value: 300, origin: "organization" }),
    entry("spec.limits.signIn.sessionIdleMinutes", "signIn", { security: true, default: 60, min: 5, max: 120 }),
    entry("spec.limits.agents.spendPerDay", "agents", {}),
  ],
  projects: [
    {
      project: "helsinki",
      origin: "project",
      quota: { contextSpaces: { limit: 12, used: 3 }, agentRunsPerDay: { limit: null, used: null } },
    },
  ],
};

/** An organization administrator (UI-75, PF-03): the page opens only for approve and delete on RoleBinding. */
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: { kinds: ["RoleBinding", "Organization"], verbs: ["propose", "approve", "delete"] },
    },
  ],
} as unknown as Effective;

function renderSettings(limits: unknown = LIMITS) {
  return renderPage(<OrganizationPage tab="settings" anchor="helsinki" />, {
    path: "/organization/settings",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
      if (url.pathname === "/api/v1/organization/limits") return json(limits);
      return undefined;
    },
  });
}

const rowOf = (label: string) => screen.getByRole("row", { name: new RegExp(label.replace(/[()]/g, "\\$&")) });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("Organization settings: policies and limits", () => {
  it("shows one section per catalog section, then the quota of each project", async () => {
    renderSettings();
    for (const section of SECTIONS) {
      expect(await screen.findByRole("heading", { level: 3, name: en.organization.limits.section[section] })).toBeInTheDocument();
    }
    expect(screen.getByRole("heading", { level: 3, name: en.organization.limits.perProject })).toBeInTheDocument();
  });

  it("gives each entry its value in force, its range and its hint", async () => {
    renderSettings();
    const limit = en.organization.limit;
    const web = await screen.findByRole("row", { name: new RegExp(limit.limits.edge.requestsPerMinute.web.label) });
    expect(within(web).getByText("300")).toBeInTheDocument();
    expect(within(web).getByText("10 – 6000")).toBeInTheDocument();
    expect(within(web).getByText(limit.limits.edge.requestsPerMinute.web.hint)).toBeInTheDocument();

    // Not set: the default is what holds, and the bound the operator tightened.
    const idle = rowOf(limit.limits.signIn.sessionIdleMinutes.label);
    expect(within(idle).getByText("60 (default)")).toBeInTheDocument();
    expect(within(idle).getByText("5 – 120")).toBeInTheDocument();
    // No default and no ceiling: said as such, never a blank.
    const spend = rowOf(limit.limits.agents.spendPerDay.label);
    expect(within(spend).getByText(en.organization.limits.noLimit)).toBeInTheDocument();
    expect(within(spend).getByText("at least 0")).toBeInTheDocument();

    const publicApps = rowOf(en.organization.policy.publicApps);
    expect(within(publicApps).getByText(en.organization.policy.publicApps_refused)).toBeInTheDocument();
    const models = rowOf(en.organization.policy.models);
    expect(within(models).getByText(en.organization.policy.everyModel)).toBeInTheDocument();
  });

  it("shows whose quota each project runs on and what it uses", async () => {
    renderSettings();
    const helsinki = await screen.findByRole("row", { name: /helsinki/ });
    expect(within(helsinki).getByText(en.organization.limits.origin.project)).toBeInTheDocument();
    expect(within(helsinki).getByText("3 of 12")).toBeInTheDocument();
    expect(within(helsinki).getAllByText(en.organization.limits.noLimit).length).toBeGreaterThan(0);
  });

  // T-4831 (D-settings): a TableRow inside TableHead nested a second <tr>, and the header cells
  // stood off the body's columns. One header row, as many header cells as each row has cells.
  it("lines each table's header up with its rows", async () => {
    renderSettings();
    await screen.findByRole("row", { name: /helsinki/ });
    const tables = screen.getAllByRole("table");
    expect(tables.length).toBe(SECTIONS.length + 1);
    for (const table of tables) {
      expect(table.querySelectorAll("thead tr")).toHaveLength(1);
      expect(table.querySelectorAll("thead tr tr")).toHaveLength(0);
      const headers = table.querySelectorAll("thead th").length;
      for (const row of table.querySelectorAll("tbody tr")) {
        const cells = [...row.children].reduce((sum, cell) => sum + Number(cell.getAttribute("colspan") ?? 1), 0);
        expect(cells).toBe(headers);
      }
    }
  });

  it("says why when the limits cannot be read, and keeps the rest of the page", async () => {
    renderPage(<OrganizationPage tab="settings" anchor="helsinki" />, {
      path: "/organization/settings",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
        if (url.pathname === "/api/v1/organization/limits") {
          return json({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "limits are down" }, 503);
        }
        return undefined;
      },
    });
    expect(await screen.findByText("limits are down")).toBeInTheDocument();
    expect(screen.getAllByText("hel.fi").length).toBeGreaterThan(0);
  });

  it("says so when the person reads no project, and the catalog still stands", async () => {
    renderPage(<OrganizationLimitsView limits={{ entries: LIMITS.entries, projects: [] }} />, { answer: () => undefined });
    expect(await screen.findByText(en.organization.limits.noProjects)).toBeInTheDocument();
    expect(screen.getByText(en.organization.limit.limits.agents.spendPerDay.label)).toBeInTheDocument();
  });

  it("has no axe violations", async () => {
    const { container } = renderSettings();
    await screen.findByRole("heading", { level: 3, name: en.organization.limits.perProject });
    await expectNoAxeViolations(container);
  });
});

describe("the Organization form", () => {
  const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, options);
  const errorsOf = (data: unknown) =>
    validator
      .validateFormData(data, organizationSchema(t, LIMITS.entries))
      .errors.map((error) => `${error.property} ${error.message}`);
  const base = { domain: "hel.fi", locales: ["en"], defaultLocale: "en" };

  it("holds a number to the operator's bound: above the ceiling is refused, inside it passes", () => {
    expect(errorsOf({ ...base, projects: { quota: { contextSpaces: 50 } } }).join("\n")).toMatch(/contextSpaces.*20/);
    expect(errorsOf({ ...base, limits: { signIn: { sessionIdleMinutes: 2 } } }).join("\n")).toMatch(/sessionIdleMinutes.*5/);
    expect(errorsOf({ ...base, projects: { quota: { contextSpaces: 20 } }, limits: { signIn: { sessionIdleMinutes: 120 } } })).toEqual([]);
  });

  it("explains each field with its hint, its default and its bound", () => {
    const schema = organizationSchema(t, LIMITS.entries) as {
      properties: { limits: { properties: { signIn: { properties: { sessionIdleMinutes: { title: string; description: string } } } } } };
    };
    const idle = schema.properties.limits.properties.signIn.properties.sessionIdleMinutes;
    expect(idle.title).toBe(en.organization.limit.limits.signIn.sessionIdleMinutes.label);
    expect(idle.description).toContain(en.organization.limit.limits.signIn.sessionIdleMinutes.hint);
    expect(idle.description).toContain("The default is 60.");
    expect(idle.description).toContain("Allowed: 5 – 120.");
  });

  it("refuses a public-app policy the platform does not know", () => {
    expect(errorsOf({ ...base, policies: { apps: { public: "sometimes" } } }).join("\n")).toMatch(/public/);
    expect(errorsOf({ ...base, policies: { apps: { public: "refused" } } })).toEqual([]);
  });
});
