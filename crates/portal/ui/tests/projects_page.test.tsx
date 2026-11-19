/**
 * T-7380: a person with no project who may open one lands on the Organization's projects page
 * and its "Create your first project"; a member who may not goes to the catalogue as before; a
 * person with a project goes to its home. `/projects` lists the projects with New project, and
 * the member shell offers Projects and Administration only to whom they open.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/projects/ProjectsPage.tsx,
// src/api/permissions.ts homeOf and useMayOpenProject, src/components/layout/MemberFrame.tsx; each
// confirmed by making the module throw and watching this file go red.
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { homeOf } from "../src/api/permissions";
import { asOData, doorAsRest } from "./odata";
import { expectNoAxeViolations } from "./page_contract";

const REFUSED = "opening a project here needs propose on Project, which org-admin holds (PF-65)";

const ORG_ADMIN_GRANT = {
  role: "org-admin",
  binding: "bbsk-admins",
  organization: "bbsk",
  rule: { kinds: ["RoleBinding", "Group", "Organization", "Person", "Project"], verbs: ["read", "propose", "approve"] },
};

interface World {
  projects: string[];
  creation: { allowed: boolean; reason?: string };
  administrator?: boolean;
}

function start(path: string, world: World) {
  const posted: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const door = await doorAsRest(request);
      const url = new URL(door ? door.url : request.url);
      const method = (door ?? request).method;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      const answered = async (): Promise<Response> => {
        if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "marek", name: "Marek", roles: [] });
        if (url.pathname.endsWith("/permissions/me")) {
          return json({
            project: "org",
            bootstrap: false,
            grants: world.administrator ? [ORG_ADMIN_GRANT] : [],
            projects: { creation: world.creation },
          });
        }
        if (method === "POST" && url.pathname === "/api/v1/projects") {
          posted.push(await (door ?? request).clone().json());
          return json(
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Change",
              metadata: { name: "chg-0000002a", namespace: "doprava" },
              status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2, update: 0, delete: 0 } },
            },
            202,
          );
        }
        if (url.pathname === "/api/v1/projects") {
          return json({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "ProjectList",
            items: world.projects.map((name) => ({ name, sample: false, organization: "bbsk" })),
            organizations: [],
          });
        }
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      };
      return door ? asOData(await answered()) : answered();
    }),
  );
  window.history.pushState({}, "", path);
  const result = render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...result, posted };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("where / sends a signed-in person (T-7380)", () => {
  it("decides by the project, then by the right to open one", () => {
    const allowed = { known: true, allowed: true };
    const refused = { known: true, allowed: false };
    expect(homeOf("doprava", true, refused)).toEqual({ project: "doprava" });
    expect(homeOf(undefined, true, allowed)).toBe("/projects");
    expect(homeOf(undefined, true, refused)).toBe("/catalogue");
    expect(homeOf(undefined, true, { known: false, allowed: false })).toBe("wait");
    expect(homeOf(undefined, false, allowed)).toBeUndefined();
  });

  it("opens the first project from the Organization home when the person has none and may open one", async () => {
    const { posted, container } = start("/", { projects: [], creation: { allowed: true }, administrator: true });
    expect(await screen.findByRole("heading", { level: 2, name: en.projectsPage.firstTitle })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects");
    expect(screen.getByRole("heading", { level: 1, name: en.projectsPage.title })).toBeInTheDocument();
    // The member shell offers the projects and the person's own Administration.
    expect(screen.getByRole("link", { name: en.member.projects })).toHaveAttribute("href", "/projects");
    expect(screen.getByRole("link", { name: en.member.administration })).toHaveAttribute("href", "/organization/members");
    await expectNoAxeViolations(container);

    await userEvent.click(screen.getByRole("button", { name: en.projectsPage.first }));
    const dialog = await screen.findByRole("dialog", { name: en.projects.newTitle });
    expect(dialog).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(new RegExp(en.projects.nameLabel)), "doprava");
    await userEvent.click(screen.getByRole("button", { name: en.projects.open }));
    await waitFor(() => expect(posted).toEqual([expect.objectContaining({ name: "doprava" })]));
  });

  it("keeps the catalogue as home for a member who may not open a project", async () => {
    start("/", { projects: [], creation: { allowed: false, reason: REFUSED } });
    await waitFor(() => expect(window.location.pathname).toBe("/catalogue"));
    expect(screen.queryByRole("link", { name: en.member.projects })).toBeNull();
    expect(screen.queryByRole("link", { name: en.member.administration })).toBeNull();
  });

  it("tells a member on /projects who opens projects and the way to the catalogue", async () => {
    start("/projects", { projects: [], creation: { allowed: false, reason: REFUSED } });
    expect(await screen.findByRole("heading", { level: 2, name: en.projectsPage.noneTitle })).toBeInTheDocument();
    expect(screen.getByText(new RegExp(REFUSED.replace(/[()]/g, "\\$&")))).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.projectsPage.toCatalogue })).toHaveAttribute("href", "/catalogue");
    expect(screen.queryByRole("button", { name: en.projectsPage.first })).toBeNull();
  });

  it("sends a person with a project to its home, as before", async () => {
    start("/", { projects: ["doprava"], creation: { allowed: true } });
    await waitFor(() => expect(window.location.pathname).toBe("/projects/doprava/home"));
  });
});

describe("the projects page (T-7380)", () => {
  it("lists each project as a way into its shell, with New project", async () => {
    start("/projects", { projects: ["doprava", "ovzdusie"], creation: { allowed: true } });
    const listed = within(await screen.findByRole("list", { name: en.projectsPage.title }));
    expect(listed.getByRole("link", { name: /ovzdusie/ })).toHaveAttribute("href", "/projects/ovzdusie/home");
    expect(listed.getByRole("link", { name: /doprava/ })).toHaveAttribute("href", "/projects/doprava/home");
    expect(screen.queryByText(en.app.notFound.title)).toBeNull();
    expect(within(screen.getByRole("main")).getByRole("button", { name: en.projects.new })).toBeInTheDocument();
  });
});
