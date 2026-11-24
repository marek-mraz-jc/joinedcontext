/**
 * T-7588: Organization → Settings, the cases organization_page leaves out: the manifest the form
 * proposes (empty model and service lists, an untyped publisher), projects anyone may open, the
 * appearance's links by label, the home page's Dashboard choices, and each of the page's three
 * reads failing with a Retry that reads it again.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { OrganizationSettings, toOrganization } from "../src/pages/organization/OrganizationSettings";
import { json, list, problem, renderPage } from "./page_contract";

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "hel", namespace: "org" },
  spec: {
    domain: "hel.fi",
    projects: { creation: "anyone" },
    appearance: {
      logoSvg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>',
      navLinks: [
        { label: "Open data", url: "/opendata" },
        { label: "Help", url: "https://hel.fi/help" },
      ],
    },
  },
};

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["Organization", "RoleBinding"], verbs: ["read", "propose", "approve"] } }],
};

type Read = "organizations" | "limits" | "mode";

function render(failFirst: Read[] = []) {
  const reads: Record<Read | "widgets", number> = { organizations: 0, limits: 0, mode: 0, widgets: 0 };
  const failing = (read: Read) => failFirst.includes(read) && reads[read] === 1;
  renderPage(<OrganizationSettings />, {
    path: "/organization/settings",
    answer: (url) => {
      const path = url.pathname;
      if (path.endsWith("/permissions/me")) return json(ADMIN);
      if (path === "/api/v1/projects/org/organizations") {
        reads.organizations += 1;
        return failing("organizations") ? problem(503, "the manifest store is restarting") : json(list([ORGANIZATION]));
      }
      if (path === "/api/v1/organization/limits") {
        reads.limits += 1;
        return failing("limits") ? problem(503, "the limits are not loaded yet") : json({ entries: [], projects: [] });
      }
      if (path === "/api/v1/organization/approval-mode") {
        reads.mode += 1;
        return failing("mode") ? problem(503, "the approval mode is unknown") : json({ organization: "hel", singlePerson: false, peopleWithARole: 3 });
      }
      if (path === "/api/v1/organization/widgets/options") {
        reads.widgets += 1;
        return json({ options: [{ reference: "helsinki/air", name: "air", title: { en: "Air quality" }, public: true }] });
      }
      return undefined;
    },
  });
  return reads;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Organization → Settings, the page", () => {
  it("says anyone may open a project, names the links by label and reads the home page's Dashboards", async () => {
    const reads = render();
    expect(await screen.findByText(en.organization.settings.creationAnyone)).toBeInTheDocument();
    expect(screen.getByText("Open data, Help")).toBeInTheDocument();
    // The pasted logo is drawn as an image, never as inline markup.
    const logo = screen.getByRole("img", { name: en.organization.field.appearance.logoSvg });
    expect(logo.getAttribute("src")).toMatch(/^data:image\/svg\+xml;charset=utf-8,%3Csvg/);
    await waitFor(() => expect(reads.widgets).toBe(1));
  });

  for (const [read, reason] of [
    ["organizations", "the manifest store is restarting"],
    ["limits", "the limits are not loaded yet"],
    ["mode", "the approval mode is unknown"],
  ] as const) {
    it(`says why the ${read} read failed and reads it again on Retry`, async () => {
      const user = userEvent.setup();
      const reads = render([read]);
      expect((await screen.findAllByText(new RegExp(reason))).length).toBeGreaterThan(0);
      await user.click(screen.getAllByRole("button", { name: en.app.error.retry }).at(-1) as HTMLElement);
      await waitFor(() => expect(reads[read]).toBe(2));
      await waitFor(() => expect(screen.queryAllByText(new RegExp(reason))).toEqual([]));
    });
  }
});

describe("the Organization manifest the form proposes", () => {
  const stored = { ...ORGANIZATION, status: { phase: "Live" } };
  const specOf = (form: Record<string, unknown>) => (toOrganization(form, stored) as { spec: Record<string, unknown> }).spec;

  it("keeps the models typed, dropping blank lines", () => {
    expect(specOf({ policies: { agents: { models: ["claude-opus-5-5", " ", 7] } } }).policies).toEqual({
      agents: { models: ["claude-opus-5-5"] },
    });
  });

  it("writes no model list when none is typed, which is every model", () => {
    expect(specOf({ policies: { agents: { models: [""], maxRunMinutes: 10 } } }).policies).toEqual({ agents: { maxRunMinutes: 10 } });
    expect(specOf({ policies: { agents: { maxRunMinutes: 10 } } }).policies).toEqual({ agents: { maxRunMinutes: 10 } });
  });

  it("writes no service list when no box is ticked, which is the default list", () => {
    expect(specOf({ policies: { apps: { services: [], storage: 1 } } }).policies).toEqual({ apps: { storage: 1 } });
  });

  it("drops a publisher nobody typed and keeps one with an address", () => {
    expect(specOf({ catalog: { publisher: { name: {} } } }).catalog).toBeUndefined();
    expect(specOf({ catalog: { publisher: { uri: "https://hel.fi" } } }).catalog).toEqual({ publisher: { uri: "https://hel.fi" } });
    expect(specOf({ catalog: { publisher: {}, theme: "x" } }).catalog).toEqual({ publisher: {}, theme: "x" });
  });

  it("never sends the status back", () => {
    expect(toOrganization({}, stored)).not.toHaveProperty("status");
    expect(toOrganization({ domain: "hel.fi" }, null)).toEqual({ spec: { domain: "hel.fi" } });
  });
});
