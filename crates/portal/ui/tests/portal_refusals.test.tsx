/**
 * What a person the server refuses meets, held to the server's answers (jc-portal-refusals, PF-51,
 * PF-59, PF-61, T-5533): a form is offered only once `permissions/me` says so, never while it is on
 * its way or after its 404; a failed read for another reason (500) keeps today's offer, since the
 * API re-checks every write. An outsider's refusal for a hidden project reads the same as for a
 * project that does not exist, and Administration refuses inside the Portal's own frame.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { offered } from "../src/api/permissions";
import { ProjectHidden } from "../src/components/ProjectHidden";
import type { Effective } from "../src/api/permissions";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

const ENDPOINT_EDITOR = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "endpoint-editor", binding: "editors", rule: { kinds: ["Endpoint"], verbs: ["propose"] } }],
};
const PROPOSE = /^Propose change$/;
const BOOTSTRAP = { project: "helsinki", bootstrap: true, grants: [] };
const notFound = (project: string) => problem(404, `project '${project}' not found`);

afterEach(() => {
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
  window.localStorage.clear();
});

describe("offered", () => {
  const doc = ENDPOINT_EDITOR as unknown as Effective;
  const yes = () => true;

  it("offers nothing while the first read is on its way, or after its 404", () => {
    expect(offered({ data: undefined, checking: true, denied: false }, yes)).toBe(false);
    expect(offered({ data: undefined, checking: false, denied: true }, yes)).toBe(false);
  });

  it("decides by the document once it came, and offers after another failure", () => {
    expect(offered({ data: doc, checking: false, denied: false }, (e) => e === doc)).toBe(true);
    expect(offered({ data: doc, checking: false, denied: false }, () => false)).toBe(false);
    expect(offered({ data: undefined, checking: false, denied: false }, yes)).toBe(true);
  });
});

describe("ProjectHidden", () => {
  it("says the project is not open, whether or not it exists, and leads to the start", async () => {
    const router = createRouter({
      routeTree: createRootRoute({ component: ProjectHidden }),
      history: createMemoryHistory({ initialEntries: ["/"] }),
    });
    render(<RouterProvider router={router} />);
    const said = (await screen.findByText(en.form.projectHidden)).closest("[role=status]") as HTMLElement;
    expect(within(said).getByRole("link", { name: en.app.notFound.home })).toHaveAttribute("href", "/");
    expect(said.textContent).not.toMatch(/not found|exist(s|ed)? in/);
  });
});

/** The routed create form of an Endpoint, as an outsider, a role, or a failed read meets it. */
async function newEndpoint(project: string, permissionsFor: (project: string) => Response | Promise<Response> | undefined) {
  return renderRoute({
    path: `/projects/${project}/endpoints/new`,
    projects: () => jsonResponse(list([])),
    permissionsFor,
    // Every read of a project out of reach answers the 404 of an unknown one (PF-59).
    answer: (path) => (path.startsWith(`/api/v1/projects/${project}/`) ? notFound(project) : undefined),
  });
}

describe("the create form of a project out of reach", () => {
  it("refuses an outsider in words and offers no control that writes", async () => {
    await newEndpoint("zilina", () => notFound("zilina"));
    // Not "your role lets you look": an outsider holds no role here and reads nothing.
    const refusal = (await screen.findByText(en.form.projectHidden)).closest("[role=status]") as HTMLElement;
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
    expect(within(refusal).getByRole("link", { name: en.app.notFound.home })).toHaveAttribute("href", "/");
    expect(screen.queryByRole("button", { name: PROPOSE })).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("refuses a hidden project in the very words of a project that does not exist", async () => {
    const hidden = await newEndpoint("zilina", () => notFound("zilina"));
    await screen.findByText(en.form.projectHidden);
    const main = (await screen.findByRole("main")).textContent?.replaceAll("zilina", "P");
    hidden.unmount();
    vi.unstubAllGlobals();
    await newEndpoint("nowhere", () => notFound("nowhere"));
    await screen.findByText(en.form.projectHidden);
    expect((await screen.findByRole("main")).textContent?.replaceAll("nowhere", "P")).toBe(main);
  });

  it("holds the form while the permissions are on their way", async () => {
    await newEndpoint("helsinki", () => new Promise<Response>(() => {}));
    const propose = await screen.findByRole("button", { name: PROPOSE });
    expect(propose).toHaveAttribute("aria-disabled", "true");
    expect(propose).toHaveAccessibleDescription(en.permissions.checking);
    // Said where the form starts too, not only beside its submit.
    expect(screen.getAllByText(en.permissions.checking).length).toBeGreaterThan(1);
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
  });

  it("keeps the form after a 500 on the permissions: the API re-checks the write", async () => {
    await newEndpoint("helsinki", () => problem(500, "the store is away"));
    const propose = await screen.findByRole("button", { name: PROPOSE });
    await waitFor(() => expect(propose).not.toHaveAccessibleDescription(en.permissions.checking));
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
    expect(propose).not.toHaveAccessibleDescription(/Endpoint/);
    // The person is told the check failed and the server checks again.
    expect(screen.getByText(en.permissions.unchecked)).toBeInTheDocument();
  });

  it("refuses a role that proposes nothing here with a way back to the list", async () => {
    await newEndpoint("helsinki", () =>
      jsonResponse({ project: "helsinki", bootstrap: false, grants: [{ role: "viewer", binding: "v", rule: { kinds: ["*"], verbs: ["read"] } }] }),
    );
    const refusal = (await screen.findByText(en.form.mayNotCreate)).closest("[role=status]")?.parentElement as HTMLElement;
    expect(within(refusal).getAllByRole("button", { name: en.form.backToList }).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: PROPOSE })).toBeNull();
  });

  it("offers the bootstrap administrator the whole form, with no refusal", async () => {
    await newEndpoint("helsinki", () => jsonResponse(BOOTSTRAP));
    const propose = await screen.findByRole("button", { name: PROPOSE });
    await waitFor(() => expect(propose).not.toHaveAccessibleDescription(en.permissions.checking));
    expect(propose).not.toHaveAccessibleDescription(/Endpoint/);
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
    expect(screen.queryByText(en.permissions.unchecked)).toBeNull();
  });
});

describe("a list the person may not read", () => {
  it("is a calm refusal in their words, with nothing to quote or report", async () => {
    await renderRoute({
      path: "/projects/zilina/endpoints",
      projects: () => jsonResponse(list([])),
      permissionsFor: () => notFound("zilina"),
      answer: (path) =>
        path.startsWith("/api/v1/projects/zilina/")
          ? problem(404, `plural '${path.split("/")[5]}' not found in project 'zilina'`)
          : undefined,
    });
    const refusal = (await screen.findByText(en.app.listHidden.replace("{project}", "zilina"))).closest("[role=status]");
    expect(refusal).not.toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText(en.app.error.reference)).toBeNull();
  });
});

// UI judge 2026-11-09 (outsider): Copies showed the raw 404 sentence with a Retry that cannot
// help, and Explore two empty pickers with no word at all.
describe("a project page an outsider opens", () => {
  const outsider = (path: string) =>
    renderRoute({
      path,
      projects: () => jsonResponse(list([])),
      permissionsFor: () => notFound("zilina"),
      answer: (asked) => (asked.startsWith("/api/v1/projects/zilina/") ? notFound("zilina") : undefined),
    });

  it("Copies says the project is not open, with no retry and no raw sentence", async () => {
    await outsider("/projects/zilina/workspaces");
    expect(await screen.findByText(en.form.projectHidden)).toBeInTheDocument();
    expect(screen.queryByText(/not found/)).toBeNull();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it("Explore says the project is not open instead of two empty pickers", async () => {
    await outsider("/projects/zilina/explore");
    expect(await screen.findByText(en.form.projectHidden)).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: en.explore.space })).toBeNull();
  });
});

describe("the catalogue form of the CKAN page", () => {
  async function ckan(permissionsFor: (project: string) => Response | Promise<Response> | undefined) {
    return renderRoute({ path: "/projects/helsinki/ckan", permissionsFor });
  }

  it("is refused to an outsider in one sentence, with no section that fails or writes", async () => {
    await renderRoute({
      path: "/projects/helsinki/ckan",
      permissionsFor: () => notFound("helsinki"),
      answer: (path) => (path.startsWith("/api/v1/projects/helsinki/") ? notFound("helsinki") : undefined),
    });
    expect(await screen.findByText(en.form.projectHidden)).toBeInTheDocument();
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
    expect(screen.queryByText(/not found/)).toBeNull();
    expect(screen.queryByRole("button", { name: en.ckan.instances.propose })).toBeNull();
  });

  it("is neither offered nor refused while the permissions are on their way, and says so", async () => {
    await ckan(() => new Promise<Response>(() => {}));
    await screen.findByRole("heading", { level: 1 });
    // Said at the top in its own status line, not only as a disabled control's hidden reason.
    expect(screen.getAllByRole("status").some((el) => el.textContent?.includes(en.permissions.checking))).toBe(true);
    expect(screen.queryByRole("button", { name: en.ckan.instances.propose })).toBeNull();
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
  });

  it("stays offered after a 500 on the permissions", async () => {
    await ckan(() => problem(500, "the store is away"));
    expect(await screen.findByRole("button", { name: en.ckan.instances.propose })).toBeInTheDocument();
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
  });
});

describe("Administration for a person who reads no project (PF-61)", () => {
  async function outsider(path: string) {
    return renderRoute({
      path,
      projects: () => jsonResponse(list([])),
      permissionsFor: () => notFound("org"),
      answer: (p) => (p.startsWith("/api/v1/projects/org/") || p === "/api/v1/endpoints" ? notFound("org") : undefined),
    });
  }

  it.each(["/organization/members", "/organization/groups", "/endpoints"])(
    "refuses an outsider on %s inside the Portal's frame",
    async (path) => {
      await outsider(path);
      expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
      expect(screen.getByRole("heading", { level: 1, name: en.organization.title })).toBeInTheDocument();
      const nav = screen.getAllByRole("navigation");
      expect(nav.length).toBeGreaterThan(0);
      expect(screen.getByRole("link", { name: new RegExp(en.organization.nav.toCatalogue) })).toHaveAttribute("href", "/catalogue");
      expect(screen.queryByText(en.projects.empty.title)).toBeNull();
      // The organization's namespace is no project to go back to.
      expect(screen.queryByRole("link", { name: en.askAdmins.back })).toBeNull();
      expect(window.localStorage.getItem("jc.lastProject")).not.toBe("org");
    },
  );

  it("opens the page for an administrator of an installation with no project yet", async () => {
    await renderRoute({ path: "/organization/members", projects: () => jsonResponse(list([])), permissionsFor: () => jsonResponse(BOOTSTRAP) });
    expect(await screen.findByRole("heading", { level: 1, name: en.organization.title })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText(en.organization.adminOnly)).toBeNull());
    expect(screen.queryByText(en.projects.empty.title)).toBeNull();
  });
});

describe("an Organization's reader on its Administration", () => {
  // Every reader of zilina holds a `read` on its Organization manifest narrowed to zilina (the
  // server's own-settings grant); only a grant that proposes or approves administers it.
  const READER = {
    project: "org",
    bootstrap: false,
    grants: [{ role: "viewer", binding: "zilina-viewers", organization: "zilina", rule: { kinds: ["Organization"], verbs: ["read"] } }],
  };

  it.each(["/organization/groups", "/organization/members"])("refuses %s in words and reads no list the API refuses", async (path) => {
    const page = await renderRoute({
      path,
      projects: () => jsonResponse(list([{ name: "zilina" }])),
      permissionsFor: (project) => (project === "org" ? jsonResponse(READER) : undefined),
      answer: (p) => (p.startsWith("/api/v1/projects/org/") && !p.endsWith("/organizations") ? notFound("org") : undefined),
    });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.queryByText(/not found/)).toBeNull();
    expect(page.calls()).not.toContain("GET /api/v1/projects/org/groups");
    expect(page.calls()).not.toContain("GET /api/v1/projects/org/rolebindings");
  });
});

describe("Project settings, General, for a member who may not read the organization's repository", () => {
  it("reads no project manifest it would be refused, and leaves nothing loading", async () => {
    const page = await renderRoute({
      path: "/projects/helsinki/settings/general",
      projects: () => jsonResponse(list([{ name: "helsinki", title: "City of Helsinki" }])),
      permissions: { project: "helsinki", bootstrap: false, grants: [{ role: "viewer", binding: "v", rule: { kinds: ["*"], verbs: ["read"] } }] },
      permissionsFor: (project) => (project === "org" ? notFound("org") : undefined),
      answer: (path) => (path.startsWith("/api/v1/projects/org/projects/") ? problem(404, "resource 'helsinki' not found in project 'org'") : undefined),
    });
    const general = await screen.findByRole("heading", { level: 2, name: en.projectSettings.general.title });
    const section = general.closest("section") as HTMLElement;
    await waitFor(() => expect(within(section).queryByText(en.app.loading)).toBeNull());
    expect(within(section).getByText("City of Helsinki")).toBeInTheDocument();
    expect(within(section).getAllByText(en.projectSettings.general.unknown).length).toBeGreaterThan(0);
    expect(screen.queryByText(/not found in project/)).toBeNull();
    expect(page.calls()).not.toContain("GET /api/v1/projects/org/projects/helsinki");
  });
});
