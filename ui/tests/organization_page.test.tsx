/**
 * T-2605: the Organization page (UI-75, Architecture/09 §14.1; PF-41, PF-56, PF-59, PF-61, PF-65,
 * PF-77, PF-78, UI-44).
 *
 * One tab per concern at its own address, every write a proposed change. What each tab owns is
 * asserted here: the Settings tab reads the manifest in words and edits it through the kind's
 * form, Members is listed only to whoever reads `RoleBinding` at organization scope (and nothing
 * is fetched for anybody else), Roles lists the organization's roles with the seeded taxonomy
 * marked, and Projects lists the projects with a deletion that shows its cascade and wants the
 * name typed back.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { mayRead } from "../src/api/permissions";
import { OrganizationApplications } from "../src/pages/organization/OrganizationApplications";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import type { OrganizationTab } from "../src/pages/organization/OrganizationPage";
import { fromOrganization, toOrganization } from "../src/pages/organization/OrganizationSettings";
import { DeleteProjectDialog } from "../src/components/DeleteProjectDialog";
import { expectDenied } from "./checks";
import { expectNoAxeViolations, json, list, renderPage } from "./page_contract";

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "hel", namespace: "org" },
  spec: {
    domain: "hel.fi",
    gitRepositoryUrl: "https://git.hel.fi/joinedcontext/configuration.git",
    locales: ["fi", "en"],
    defaultLocale: "fi",
    contacts: [{ role: "data-protection", name: "Tietosuoja", email: "tietosuoja@hel.fi" }],
    projects: { creation: "group:project-leads", visibility: "members", nameCooldownDays: 14 },
  },
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

const BINDINGS = [
  binding("admins", "ida@hel.fi", "org-admin", { organization: "hel" }),
  binding("helsinki-steward", "eva@hel.fi", "steward", { project: "helsinki" }),
  binding("bikes-editor", "jan@hel.fi", "pipeline-editor", { contextSpace: "bikes" }),
];

function role(name: string, namespace: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Role",
    metadata: { name, namespace },
    spec: { rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }] },
  };
}

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: { kinds: ["RoleBinding", "Role", "Group", "Organization", "Project"], verbs: ["propose", "approve", "delete"] },
    },
  ],
} as unknown as Effective;

const VIEWER = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "viewer", binding: "everyone", rule: { kinds: ["Pipeline", "Project"], verbs: ["read"] } }],
} as unknown as Effective;

/** Administers the organization and holds nothing else: reads no binding, deletes no project. */
const BARE_ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    { role: "viewer", binding: "everyone", rule: { kinds: ["Pipeline", "Project"], verbs: ["read"] } },
    { role: "org-approver", binding: "approvers", rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] } },
  ],
} as unknown as Effective;

interface World {
  permissions?: Effective;
}

function renderAt(tab: OrganizationTab, world: World = {}) {
  const { permissions = ADMIN } = world;
  const requests: string[] = [];
  const result = renderPage(<OrganizationPage tab={tab} anchor="helsinki" />, {
    path: `/organization/${tab}`,
    answer: (url, request) => {
      requests.push(`${request.method} ${url.pathname}`);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/projects") return json(list([{ name: "helsinki" }, { name: "espoo" }]));
      if (url.pathname === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
      if (url.pathname === "/api/v1/organization/limits") return json({ entries: [], projects: [] });
      if (url.pathname === "/api/v1/projects/org/rolebindings") return json(list(BINDINGS));
      if (url.pathname === "/api/v1/projects/org/roles") {
        return json(list([role("viewer", "org"), role("air-analyst", "org")]));
      }
      if (url.pathname === "/api/v1/projects/org/projects") {
        return json(
          list([
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Project",
              metadata: { name: "helsinki", namespace: "org", title: "Helsinki city data" },
              spec: { organizationRef: "hel" },
            },
          ]),
        );
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

describe("the Organization page", () => {
  // UI-16, T-4831: one H1, no tab strip; the sections live in the side navigation's groups.
  it("names itself once, with its group's lead, and draws no tab strip", async () => {
    renderAt("roles");
    expect(await screen.findByRole("heading", { level: 1, name: en.organization.title })).toBeInTheDocument();
    expect(await screen.findByText(en.organization.groupLead.access)).toBeInTheDocument();
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  // PF-61, T-2877, UI-75: every endpoint of every project is an administration view. An
  // administrator has the tab and the table; anyone else meets the page's notice, no tab, and no
  // request for the list.
  it("gives the Endpoints tab to an administrator and to nobody else", async () => {
    const admin = renderAt("endpoints");
    expect(await screen.findByRole("table", { name: en.allEndpoints.title })).toBeInTheDocument();
    expect(admin.requests).toContain("GET /api/v1/endpoints");
    cleanup();

    const viewer = renderAt("endpoints", { permissions: VIEWER });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: en.allEndpoints.title })).not.toBeInTheDocument();
    expect(viewer.requests).not.toContain("GET /api/v1/endpoints");
    cleanup();

    // Approving bindings without deleting them is not administering the organization (PF-03).
    renderAt("settings", {
      permissions: {
        project: "org",
        bootstrap: false,
        grants: [{ role: "binding-approver", binding: "approvers", rule: { kinds: ["RoleBinding"], verbs: ["approve"] } }],
      } as unknown as Effective,
    });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 2 })).toBeNull();
  });

  // T-4831 (D-environments): the API's 404 for a kind the role reads none of was the Portal's
  // "This page does not exist"; it says who sees them instead, and asks the API for nothing.
  it("tells an administrator whose role reads no environments who does, instead of a missing page", async () => {
    const { requests } = renderAt("environments");
    expect(
      await screen.findByText(en.organization.kindHidden.replace("{kinds}", en.kinds.Environment.other)),
    ).toBeInTheDocument();
    expect(screen.queryByText(en.app.notFound.title)).toBeNull();
    expect(requests).not.toContain("GET /api/v1/projects/org/environments");
  });

  // T-4831 (D-agentprofiles): the refusal names who may, in words, not 'propose' on 'AgentProfile'.
  it("says in words that only an organization administrator adds agent profiles", async () => {
    renderAt("agentprofiles", {
      permissions: {
        ...ADMIN,
        grants: [
          ...(ADMIN as unknown as { grants: unknown[] }).grants,
          { role: "viewer", binding: "everyone", rule: { kinds: ["AgentProfile"], verbs: ["read"] } },
        ],
      } as unknown as Effective,
    });
    const add = await screen.findByRole("button", { name: en.kinds.AgentProfile.new });
    await waitFor(() =>
      expectDenied(add, "Only an organization administrator can add or change agent profiles."),
    );
    expect(document.body.textContent).not.toContain("does not permit");
    // The list says it is the organization's, not a project's.
    expect(screen.getByText(en.resourceList.leadOrganization.replace("{kind}", en.organization.tab.agentprofiles))).toBeInTheDocument();
  });

  // PF-61, PF-65, PF-78: the projects policy in plain words, not the manifest's tokens.
  it("reads the organization's settings in words", async () => {
    renderAt("settings");
    expect((await screen.findAllByText("hel.fi")).length).toBeGreaterThan(0);
    expect(screen.getByText("The members of the group project-leads open projects.")).toBeInTheDocument();
    expect(screen.getByText(en.organization.visibility.members)).toBeInTheDocument();
    expect(screen.getByText(/Data protection: Tietosuoja/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Edit hel\.fi/ })).toBeInTheDocument();
  });

  it("says so when the installation has written no Organization manifest", async () => {
    renderPage(<OrganizationPage tab="settings" anchor="helsinki" />, {
      path: "/organization/settings",
      answer: () => undefined,
    });
    expect(await screen.findByText(en.organization.settings.empty)).toBeInTheDocument();
  });

  // PF-59: the members are listed to a person who reads RoleBinding at organization scope, and
  // only the organization's own bindings are members of the organization.
  it("lists the bindings at organization scope, and no project's or space's", async () => {
    renderAt("members");
    expect(await screen.findByText("ida@hel.fi")).toBeInTheDocument();
    expect(screen.queryByText("eva@hel.fi")).not.toBeInTheDocument();
    expect(screen.queryByText("jan@hel.fi")).not.toBeInTheDocument();
  });

  // UI-75, UI-87 (T-2879): the page is an organization administrator's. Anybody else is told
  // whose it is, meets no tab, and none of its data is asked for.
  it("tells a person who does not administer the organization whose page it is, and fetches nothing of it", async () => {
    const { requests } = renderAt("projects", { permissions: VIEWER });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: en.organization.title })).toBeInTheDocument();
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.projectImport.button })).not.toBeInTheDocument();
    // The shell's own reads (permissions, branding) go; of the organization only its manifest,
    // for the contacts the refusal names (T-6423), which the server answers by the person's rights.
    const reads = requests.filter((request) => request.includes("/projects/org/") || request.endsWith("/api/v1/projects"));
    expect([...new Set(reads)].sort()).toEqual([
      "GET /api/v1/projects/org/organizations",
      "GET /api/v1/projects/org/permissions/me",
    ]);
  });

  // PF-59, security: an administrator without `read` on RoleBinding is told who can see them,
  // and the list is never fetched.
  it("tells a person who may not read the bindings who can, and fetches none", async () => {
    const { requests } = renderAt("members", { permissions: BARE_ADMIN });
    expect(await screen.findByText(en.organization.members.hidden)).toBeInTheDocument();
    expect(requests.filter((request) => request.includes("/rolebindings"))).toEqual([]);
    expect(screen.queryByText("ida@hel.fi")).not.toBeInTheDocument();
  });

  // PF-56: the organization's roles once each, the taxonomy's marked seeded, a role the
  // organization wrote itself not.
  it("lists the organization's roles once, marking the seeded taxonomy", async () => {
    const { requests } = renderAt("roles");
    const viewer = await screen.findByRole("row", { name: /viewer/ });
    expect(within(viewer).getByText(en.organization.roles.seeded)).toBeInTheDocument();
    const own = screen.getByRole("row", { name: /air-analyst/ });
    expect(within(own).queryByText(en.organization.roles.seeded)).not.toBeInTheDocument();
    expect(screen.getAllByRole("row", { name: /viewer/ })).toHaveLength(1);
    expect(requests.filter((request) => request.includes("/helsinki/roles"))).toEqual([]);
  });

  // PF-65, PF-77: every project, with its title and who is bound in it.
  it("lists the projects with their titles and how many are bound at project scope", async () => {
    renderAt("projects");
    const helsinki = await screen.findByRole("row", { name: /Helsinki city data/ });
    // T-2759: every row read "0" while the organization's members act in every project.
    await waitFor(() =>
      expect(within(helsinki).getByText("1 of its own · 1 through the organization")).toBeInTheDocument(),
    );
    const espoo = screen.getByRole("row", { name: /espoo/ });
    expect(within(espoo).getByText("None of its own · 1 through the organization")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Helsinki city data" })).toHaveAttribute(
      "href",
      "/projects/helsinki/spaces",
    );
    // Delete was the only action: the project's settings are one click away.
    expect(within(helsinki).getByRole("link", { name: en.organization.projects.settings })).toHaveAttribute(
      "href",
      "/projects/helsinki/settings",
    );
  });

  it("says the organization's members hold their roles across it, not in a project", async () => {
    renderAt("members");
    expect(await screen.findByRole("heading", { name: en.organization.members.title })).toBeInTheDocument();
    expect(screen.getByText(en.organization.members.lead)).toBeInTheDocument();
    expect(screen.queryByText(/in this project/)).toBeNull();
  });

  // UI-44: an administrator who may not delete projects meets Delete disabled with the reason,
  // and the count of bound people is not computed from bindings they may not read.
  it("keeps Delete in place for who may not delete, disabled with the reason, and shows no counts", async () => {
    const { requests } = renderAt("projects", { permissions: BARE_ADMIN });
    const helsinki = await screen.findByRole("row", { name: /Helsinki city data/ });
    await waitFor(() => {
      expectDenied(
        within(helsinki).getByRole("button", { name: "Delete project helsinki" }),
        /permit .delete. on .Project./,
      );
    });
    expect(within(helsinki).getByText(en.organization.projects.peopleHidden)).toBeInTheDocument();
    expect(requests.filter((request) => request.includes("/rolebindings"))).toEqual([]);
  });

  // UI-87 (T-2879): the whole project's export and a project's import live here.
  it("offers each project's export and the import of a project", async () => {
    const user = userEvent.setup();
    renderAt("projects");
    const helsinki = await screen.findByRole("row", { name: /Helsinki city data/ });
    await user.click(within(helsinki).getByRole("button", { name: en.organization.projects.export }));
    const exporting = await screen.findByRole("dialog");
    expect(within(exporting).getByRole("radio", { name: (name) => name.startsWith(en.export.formats.whole) })).toBeChecked();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: en.projectImport.button }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("passes axe on every tab", async () => {
    for (const tab of ["settings", "members", "roles", "projects", "applications"] as const) {
      const { container } = renderAt(tab);
      await screen.findByRole("heading", { level: 1, name: en.organization.title });
      await expectNoAxeViolations(container);
      cleanup();
    }
  });
});

function app(name: string, project: string, spec: Record<string, unknown>, title?: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { name, namespace: project, ...(title ? { title } : {}) },
    spec,
  };
}

const APPS: Record<string, unknown[]> = {
  helsinki: [
    app(
      "air-map",
      "helsinki",
      { lifecycle: "published", source: { git: { url: "https://git.hel.fi/apps/helsinki_air-map.git" } } },
      "Air quality map",
    ),
  ],
  espoo: [app("kiosk", "espoo", { lifecycle: "draft", source: { path: "apps/kiosk" } })],
};

interface Sent {
  method: string;
  path: string;
  search: string;
  form: FormData | null;
}

/** The Applications tab over two projects; `exported` and `imported` answer the two routes. */
function renderApplications(
  exported: () => Response = () =>
    new Response("PK", {
      status: 200,
      headers: { "Content-Type": "application/zip", "Content-Disposition": 'attachment; filename="air-map-app-b47f807.zip"' },
    }),
  imported: (dryRun: boolean) => Response = (dryRun) =>
    dryRun
      ? json({ repositories: [{ name: "air-map", role: "application", repository: "espoo_mapa", head: "b47f8079d2df" }] })
      : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-9", namespace: "espoo" }, status: { lane: "red", phase: "PendingApproval" } }, 202),
) {
  const sent: Sent[] = [];
  const result = renderPage(<OrganizationApplications />, {
    path: "/organization/applications",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/projects") return json(list([{ name: "helsinki" }, { name: "espoo" }]));
      const apps = /^\/api\/v1\/projects\/([^/]+)\/apps$/.exec(url.pathname);
      if (apps && request.method === "GET") return json(list(APPS[apps[1]] ?? []));
      if (url.pathname.endsWith("/export") || url.pathname.endsWith("/import")) {
        const multipart = request.headers.get("content-type")?.startsWith("multipart/form-data") ?? false;
        sent.push({
          method: request.method,
          path: url.pathname,
          search: url.search,
          form: multipart ? await request.clone().formData() : null,
        });
        return url.pathname.endsWith("/export") ? exported() : imported(url.searchParams.has("dryRun"));
      }
      return undefined;
    },
  });
  return { ...result, sent };
}

// UI-75, UI-87 (T-2879): one App of any project leaves and arrives here and nowhere else.
describe("the Applications tab", () => {
  let saved: string[] = [];
  beforeEach(() => {
    saved = [];
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: () => "blob:app", revokeObjectURL: () => undefined }));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      if (this.download) saved.push(this.download);
    });
  });

  it("lists every App of every project with its project, lifecycle and repository", async () => {
    renderApplications();
    const map = await screen.findByRole("row", { name: /Air quality map/ });
    expect(within(map).getByText("helsinki")).toBeInTheDocument();
    expect(within(map).getByText("helsinki_air-map")).toBeInTheDocument();
    const kiosk = screen.getByRole("row", { name: /kiosk/ });
    expect(within(kiosk).getByText("espoo")).toBeInTheDocument();
    expect(within(kiosk).getByText(en.organization.applications.noRepository)).toBeInTheDocument();
  });

  it("keeps Export in place for an App with no repository to bundle, disabled with the reason", async () => {
    renderApplications();
    const kiosk = await screen.findByRole("row", { name: /kiosk/ });
    expectDenied(within(kiosk).getByRole("button", { name: "Export kiosk" }), en.appExport.noRepository);
  });

  it("saves the archive under the server's name", async () => {
    const user = userEvent.setup();
    const { sent } = renderApplications();
    const map = await screen.findByRole("row", { name: /Air quality map/ });
    await user.click(within(map).getByRole("button", { name: "Export air-map" }));
    await waitFor(() => expect(saved, screen.queryByRole("alert")?.textContent ?? "").toEqual(["air-map-app-b47f807.zip"]));
    expect(sent.map((one) => `${one.method} ${one.path}`)).toEqual(["GET /api/v1/projects/helsinki/apps/air-map/export"]);
  });

  it("shows the API's refusal of an export as it comes", async () => {
    const user = userEvent.setup();
    renderApplications(() =>
      json({ title: "Forbidden", status: 403, detail: "exporting an application is for organization administrators" }, 403),
    );
    const map = await screen.findByRole("row", { name: /Air quality map/ });
    await user.click(within(map).getByRole("button", { name: "Export air-map" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("exporting an application is for organization administrators");
    expect(saved).toEqual([]);
  });

  it("imports an export into a project under a name: the check, then the check and the import", async () => {
    const user = userEvent.setup();
    const { sent } = renderApplications();
    await user.click(await screen.findByRole("button", { name: en.appImport.button }));
    const dialog = await screen.findByRole("dialog");
    const check = within(dialog).getByRole("button", { name: en.appImport.check });
    expectDenied(check, en.appImport.noFile);
    await user.upload(within(dialog).getByLabelText(en.appImport.file), new File(["PK"], "air-map-app.zip", { type: "application/zip" }));
    expectDenied(check, en.appImport.noProject);
    await user.selectOptions(within(dialog).getByLabelText(new RegExp(en.appImport.project)), "espoo");
    await user.type(within(dialog).getByLabelText(new RegExp(en.appImport.name)), "Mapa");
    expect(within(dialog).getAllByText(en.appImport.nameInvalid).length).toBeGreaterThan(0);
    expectDenied(check, en.appImport.nameInvalid);
    await user.clear(within(dialog).getByLabelText(new RegExp(en.appImport.name)));
    await user.type(within(dialog).getByLabelText(new RegExp(en.appImport.name)), "mapa");
    await user.click(check);
    expect(await within(dialog).findByText(/espoo_mapa/)).toBeInTheDocument();
    expect(sent[0].search).toBe("?format=app&dryRun=All");
    expect(sent[0].path).toBe("/api/v1/projects/espoo/import");
    expect(sent[0].form?.get("name")).toBe("mapa");
    await user.click(within(dialog).getByRole("button", { name: en.appImport.import }));
    await waitFor(() => expect(sent.map((one) => one.search)).toEqual(["?format=app&dryRun=All", "?format=app&dryRun=All", "?format=app"]));
    // The archive rides as a file part, never as text.
    expect(typeof sent[2].form?.get("file")).toBe("object");
    expect(await within(dialog).findByText(/chg-9/)).toBeInTheDocument();
  });

  it("shows the API's refusal of an import and imports nothing", async () => {
    const user = userEvent.setup();
    const { sent } = renderApplications(undefined, () =>
      json({ title: "Conflict", status: 409, detail: "the project 'espoo' has an App named 'air-map' already; import it under another name" }, 409),
    );
    await user.click(await screen.findByRole("button", { name: en.appImport.button }));
    const dialog = await screen.findByRole("dialog");
    await user.upload(within(dialog).getByLabelText(en.appImport.file), new File(["PK"], "air-map-app.zip"));
    await user.selectOptions(within(dialog).getByLabelText(new RegExp(en.appImport.project)), "espoo");
    await user.click(within(dialog).getByRole("button", { name: en.appImport.check }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("import it under another name");
    expect(sent.map((one) => one.search)).toEqual(["?format=app&dryRun=All"]);
  });
});

describe("deleting a project (PF-77, PF-78)", () => {
  function renderDelete(answer: (url: URL, request: Request) => Response | undefined) {
    const sent: string[] = [];
    renderPage(<DeleteProjectDialog project="helsinki" open onOpenChange={() => undefined} />, {
      path: "/organization/projects",
      answer: (url, request) => {
        if (request.method !== "GET") sent.push(`${request.method} ${url.pathname}`);
        return answer(url, request);
      },
    });
    return sent;
  }

  it("lists what goes with the project and the reserved name before anything is sent", async () => {
    const sent = renderDelete((url) => {
      if (url.pathname === "/api/v1/projects/helsinki") {
        return json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Project",
          metadata: {},
          spec: {},
          status: { usage: { contextSpaces: 3, apps: 1, publicEndpoints: 0 } },
        });
      }
      if (url.pathname === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
      return undefined;
    });
    expect(await screen.findByText("3 × Context Spaces")).toBeInTheDocument();
    expect(screen.getByText("1 × Apps")).toBeInTheDocument();
    expect(screen.queryByText(/Public endpoints/)).not.toBeInTheDocument();
    expect(await screen.findByText(/stays reserved for 14 days/)).toBeInTheDocument();
    expect(sent).toEqual([]);
  });

  it("is never one click: the name typed back, then one DELETE that answers with the change", async () => {
    const user = userEvent.setup();
    const sent = renderDelete((url, request) => {
      if (request.method === "DELETE" && url.pathname === "/api/v1/projects/helsinki") {
        return json(
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "chg-00000abc", namespace: "helsinki" },
            status: { lane: "red", phase: "PendingApproval" },
          },
          202,
        );
      }
      return undefined;
    });
    const propose = await screen.findByRole("button", { name: en.projectDelete.propose });
    expectDenied(propose);
    await user.click(propose);
    expect(sent).toEqual([]);
    await user.type(screen.getByRole("textbox"), "helsink");
    expectDenied(propose);
    await user.type(screen.getByRole("textbox"), "i");
    await user.click(propose);
    await waitFor(() => expect(sent).toEqual(["DELETE /api/v1/projects/helsinki"]));
  });

  it("shows the API's refusal as it comes", async () => {
    const user = userEvent.setup();
    renderDelete((_url, request) =>
      request.method === "DELETE"
        ? json(
            { title: "Conflict", status: 409, detail: "project 'helsinki' is shared with espoo/bikes: remove the SharedSpaceReference there first (PF-77)" },
            409,
          )
        : undefined,
    );
    await user.type(await screen.findByRole("textbox"), "helsinki");
    await user.click(screen.getByRole("button", { name: en.projectDelete.propose }));
    expect(await screen.findByRole("alert")).toHaveTextContent("shared with espoo/bikes");
  });
});

describe("the Organization form keeps what it does not show", () => {
  it("leaves gitRepositoryUrl out of the form and puts it back into the manifest, without status", () => {
    const form = fromOrganization(ORGANIZATION);
    expect(form).not.toHaveProperty("gitRepositoryUrl");
    const manifest = toOrganization({ ...form, defaultLocale: "en" }, ORGANIZATION) as typeof ORGANIZATION;
    expect(manifest.spec.gitRepositoryUrl).toBe(ORGANIZATION.spec.gitRepositoryUrl);
    expect(manifest.spec.defaultLocale).toBe("en");
    expect(manifest.metadata).toEqual(ORGANIZATION.metadata);
    expect(manifest).not.toHaveProperty("status");
  });
});

describe("mayRead (PF-59)", () => {
  it("reads with read or propose, not with approve alone, and waits while nothing has arrived", () => {
    const grant = (verbs: string[]) =>
      ({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["RoleBinding"], verbs } }] }) as unknown as Effective;
    expect(mayRead(grant(["read"]), "RoleBinding")).toBe(true);
    expect(mayRead(grant(["propose"]), "RoleBinding")).toBe(true);
    expect(mayRead(grant(["approve"]), "RoleBinding")).toBe(false);
    expect(mayRead(grant(["read"]), "Role")).toBe(false);
    expect(mayRead(undefined, "RoleBinding")).toBeUndefined();
    expect(mayRead({ project: "org", bootstrap: true, grants: [] } as unknown as Effective, "RoleBinding")).toBe(true);
  });
});
