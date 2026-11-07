/**
 * T-2606: Project settings tab by tab (UI-76, Architecture/09 §14.2; PF-17, PF-50, PF-52, PF-59,
 * PF-60, PF-68, PF-69, PF-77, UI-44).
 *
 * The page contract of the frame is `page_project_settings.test.tsx`; this file holds what each
 * tab decides: General edits project.yaml and keeps what its form does not show, Members holds
 * this project's and its spaces' bindings and never grants at organization scope, Roles lists and
 * writes this project's own roles with project kinds only, Your access reads the caller's own
 * grants, and Delete project is never one click.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import {
  approvesOthersOnly,
  fromProject,
  ProjectSettingsPage,
  toProject,
} from "../src/pages/projectSettings/ProjectSettingsPage";
import type { ProjectSettingsTab } from "../src/pages/projectSettings/ProjectSettingsPage";
import { NOT_IN_A_PROJECT, organizationKindsIn } from "../src/pages/access/Roles";
import { expectDenied } from "./checks";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "helsinki";

const PROJECT_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Project",
  metadata: { name: PROJECT, namespace: "org", title: "Helsinki city data", description: "Open data of the city." },
  spec: { organizationRef: "hel", quotas: { contextSpaces: 4 } },
  status: { phase: "Live" },
};

function binding(name: string, subject: string, role: string, scope: Record<string, string>) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "RoleBinding",
    metadata: { name, namespace: "org" },
    spec: { subjects: [{ user: subject }], role, scope },
  };
}

function role(name: string, namespace: string, kinds: string[]) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Role",
    metadata: { name, namespace },
    spec: { rules: [{ kinds, verbs: ["propose"] }] },
  };
}

const STEWARD = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      role: "steward",
      binding: "helsinki-stewards",
      scope: "project:helsinki",
      rule: { kinds: ["Pipeline", "DataSource", "Role", "RoleBinding"], verbs: ["propose", "approve"] },
    },
    // A steward reads the spaces the bindings name; a list it does not read is never asked for.
    { role: "space-reader", binding: "helsinki-readers", scope: "project:helsinki", rule: { kinds: ["ContextSpace"], verbs: ["read"] } },
  ],
} as unknown as Effective;

const VIEWER = {
  project: PROJECT,
  bootstrap: false,
  grants: [{ role: "viewer", binding: "viewers", scope: "organization", rule: { kinds: ["Pipeline"], verbs: ["read"] } }],
} as unknown as Effective;

function renderTab(tab: ProjectSettingsTab, permissions: Effective = STEWARD) {
  const requests: string[] = [];
  const result = renderPage(<ProjectSettingsPage project={PROJECT} tab={tab} />, {
    path: `/projects/${PROJECT}/settings/${tab}`,
    answer: (url, request) => {
      requests.push(`${request.method} ${url.pathname}`);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === `/api/v1/projects/org/projects/${PROJECT}`) return json(PROJECT_MANIFEST);
      if (url.pathname === "/api/v1/projects/org/organizations") {
        return json(
          list([
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Organization",
              metadata: { name: "hel", namespace: "org" },
              spec: { domain: "hel.fi", locales: ["fi"], defaultLocale: "fi", projects: { visibility: "members" } },
            },
          ]),
        );
      }
      if (url.pathname === "/api/v1/projects/org/rolebindings") {
        return json(
          list([
            binding("admins", "ida@hel.fi", "org-admin", { organization: "hel" }),
            binding("stewards", "eva@hel.fi", "steward", { project: PROJECT }),
            binding("bikes", "jan@hel.fi", "pipeline-author", { contextSpace: "bikes" }),
            binding("espoo", "tom@hel.fi", "steward", { project: "espoo" }),
          ]),
        );
      }
      if (url.pathname === `/api/v1/projects/${PROJECT}/spaces`) {
        return json(
          list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "bikes", namespace: PROJECT }, spec: {} }]),
        );
      }
      if (url.pathname === `/api/v1/projects/${PROJECT}/roles`) {
        return json(list([role("pipeline-author", PROJECT, ["Pipeline"])]));
      }
      if (url.pathname === "/api/v1/projects/org/roles") {
        return json(list([role("steward", "org", ["Pipeline"]), role("org-admin", "org", ["Project"])]));
      }
      if (url.pathname === `/api/v1/projects/${PROJECT}`) {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Project", metadata: {}, spec: {}, status: { usage: { contextSpaces: 2 } } });
      }
      return undefined;
    },
  });
  return { ...result, requests };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("General (PF-17, PF-61)", () => {
  it("reads the project's title, description and organization, with Edit behind the kind's form", async () => {
    renderTab("general");
    expect(await screen.findByText("Open data of the city.")).toBeInTheDocument();
    expect(screen.getByText("Helsinki city data")).toBeInTheDocument();
    expect(screen.getByText("hel.fi")).toBeInTheDocument();
    expect(screen.getByText(en.organization.visibility.members)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Edit Helsinki city data/ })).toBeInTheDocument();    // No publisher saved, so no catalogue address to hand out (T-4595).
    expect(screen.getByText(en.projectSettings.general.noCatalog)).toBeInTheDocument();
  });

  it("edits title, description and quotas and keeps organizationRef, the metadata and no status", () => {
    const form = fromProject(PROJECT_MANIFEST);
    expect(form).toEqual({
      title: "Helsinki city data",
      description: "Open data of the city.",
      quotas: { contextSpaces: 4 },
      apps: { services: [], limits: {} },
      catalog: {},
    });
    const edited = toProject(
      { title: "Helsinki", description: "  ", quotas: { contextSpaces: 3, apps: undefined } },
      PROJECT_MANIFEST,
    ) as typeof PROJECT_MANIFEST;
    expect(edited.metadata).toEqual({ name: PROJECT, namespace: "org", title: "Helsinki" });
    expect(edited.spec).toEqual({ organizationRef: "hel", quotas: { contextSpaces: 3 } });
    expect(edited).not.toHaveProperty("status");
    // Every quota cleared is no quotas block at all, not an empty one.
    expect((toProject({ title: "x", quotas: {} }, PROJECT_MANIFEST) as typeof PROJECT_MANIFEST).spec).toEqual({
      organizationRef: "hel",
    });
  });
});

describe("Members (PF-60, PF-69)", () => {
  it("holds this project's and its spaces' bindings, not the organization's or another project's", async () => {
    renderTab("members");
    expect(await screen.findByText("eva@hel.fi")).toBeInTheDocument();
    expect(await screen.findByText("jan@hel.fi")).toBeInTheDocument();
    expect(screen.queryByText("ida@hel.fi")).not.toBeInTheDocument();
    expect(screen.queryByText("tom@hel.fi")).not.toBeInTheDocument();
  });

  it("grants in the project or one of its spaces, never over the organization, with the project's roles offered", async () => {
    const user = userEvent.setup();
    renderTab("members");
    await user.click(await screen.findByRole("button", { name: en.access.roles.grant }));
    const dialog = await screen.findByRole("dialog", { name: en.access.roles.grantTitle });
    const where = within(dialog).getByLabelText(en.access.roles.whereLabel);
    await waitFor(() => expect(within(where).getAllByRole("option").length).toBe(2));
    expect(within(where).queryByRole("option", { name: en.access.roles.organization })).not.toBeInTheDocument();
    const roles = within(dialog).getByRole("combobox", { name: new RegExp(en.access.roles.roleLabel) });
    await waitFor(() => {
      expect(within(roles).getByRole("option", { name: "pipeline-author" })).toBeInTheDocument();
    });
    expect(within(roles).getByRole("option", { name: "steward" })).toBeInTheDocument();
  });
});

describe("Roles (PF-68)", () => {
  it("lists this project's own roles and not the organization's", async () => {
    const { requests } = renderTab("roles");
    expect(await screen.findByRole("row", { name: /pipeline-author/ })).toBeInTheDocument();
    expect(screen.queryByRole("row", { name: /org-admin/ })).not.toBeInTheDocument();
    expect(requests.filter((request) => request.endsWith("/org/roles"))).toEqual([]);
  });

  it("names the organization kinds a project role may not hold, each once", () => {
    expect(organizationKindsIn([{ kinds: ["Pipeline", "Role"] }, { kinds: ["RoleBinding", "Role"] }])).toEqual([
      "Role",
      "RoleBinding",
    ]);
    expect(organizationKindsIn([{ kinds: ["Pipeline"] }])).toEqual([]);
    expect(organizationKindsIn(undefined)).toEqual([]);
    expect([...NOT_IN_A_PROJECT]).toEqual(["Role", "RoleBinding", "Group", "Organization", "Project"]);
  });

  it("offers project kinds only in the new role's kind list", async () => {
    const user = userEvent.setup();
    renderTab("roles");
    await user.click(await screen.findByRole("button", { name: en.access.projectRoles.new }));
    const dialog = await screen.findByRole("dialog", { name: en.access.projectRoles.newTitle });
    // The rule list sits under Custom, behind the presets (T-4832).
    await user.click(await within(dialog).findByRole("radio", { name: "Custom" }));
    await waitFor(() => expect(within(dialog).getAllByRole("checkbox", { name: "Pipeline" }).length).toBeGreaterThan(0));
    for (const kind of NOT_IN_A_PROJECT) {
      expect(within(dialog).queryByRole("checkbox", { name: kind })).not.toBeInTheDocument();
    }
  });
});

describe("Your access (PF-50, PF-60)", () => {
  it("says approve without delete approves other people's changes only (PF-58, T-5593)", () => {
    const approve = { verbs: ["propose", "approve"], kinds: ["Endpoint", "Policy"] };
    expect(approvesOthersOnly(approve, [approve])).toBe(true);
    expect(approvesOthersOnly(approve, [approve, { verbs: ["delete"], kinds: ["Endpoint", "Policy"] }])).toBe(false);
    expect(approvesOthersOnly(approve, [approve, { verbs: ["delete"], kinds: ["*"] }])).toBe(false);
    expect(approvesOthersOnly(approve, [approve, { verbs: ["delete"], kinds: ["Endpoint"] }])).toBe(true);
    expect(approvesOthersOnly({ verbs: ["read"], kinds: ["*"] }, [])).toBe(false);
  });

  it("lists the caller's own grants with the binding and scope each came from", async () => {
    renderTab("access");
    const row = await screen.findByRole("row", { name: /steward/ });
    // Grouped in words, the four kinds one click away (T-4990).
    expect(row.querySelector("summary")?.textContent).toBe("Propose and approve: 4 kinds");
    expect(within(row).getByText(/^pipelines, data sources, roles/)).toBeInTheDocument();
    expect(within(row).getByText("binding helsinki-stewards, project:helsinki")).toBeInTheDocument();
  });
});

describe("Delete project (PF-77, UI-44)", () => {
  it("keeps Delete in place for a viewer, disabled with the reason", async () => {
    renderTab("danger", VIEWER);
    await waitFor(() => {
      expectDenied(screen.getByRole("button", { name: "Delete project helsinki" }), /permit .delete. on .Project./);
    });
  });

  it("opens the dialog that lists the cascade instead of deleting", async () => {
    const user = userEvent.setup();
    const { requests } = renderTab("danger", { ...STEWARD, bootstrap: true } as Effective);
    await user.click(await screen.findByRole("button", { name: "Delete project helsinki" }));
    expect(await screen.findByText("2 × Context Spaces")).toBeInTheDocument();
    expect(requests.filter((request) => request.startsWith("DELETE"))).toEqual([]);
  });
});
