/**
 * T-2605, UI-75: the Organization page is reached from beside the project switcher, outside any
 * project. `/organization` opens on its first tab, a tab of its own address opens that tab, and
 * an address that names no tab is the Portal's not-found page rather than an empty frame.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import userEvent from "@testing-library/user-event";
import { ORGANIZATION_GROUPS } from "../src/pages/organization/AdminNav";
import { ORGANIZATION_TABS } from "../src/pages/organization/OrganizationPage";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["portal-viewer"] };
const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] };

function renderAt(path: string, permissions: unknown = EMPTY_LIST) {
  window.history.pushState({}, "", path);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? PROJECTS
          : url.includes("/permissions/me")
            ? permissions
            : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the Organization page's addresses (T-2605)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens /organization on its Settings section", async () => {
    renderAt("/organization");
    await waitFor(() => expect(window.location.pathname).toBe("/organization/settings"));
    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    expect(await within(nav).findByRole("link", { name: en.organization.tab.settings })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  // T-4831 (owner 2026-10-20): Administration has its own sidebar, its sections in four named
  // groups, and no project switcher, since nothing on it belongs to a project.
  it("lists every section in the side navigation's four groups, with no project switcher", async () => {
    renderAt("/organization/members");
    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    for (const group of ORGANIZATION_GROUPS) {
      const list = await within(nav).findByRole("list", { name: en.organization.group[group.id] });
      expect(within(list).getAllByRole("link").map((link) => link.textContent)).toEqual(
        group.tabs.map((tab) => en.organization.tab[tab]),
      );
      for (const tab of group.tabs) {
        expect(within(list).getByRole("link", { name: en.organization.tab[tab] })).toHaveAttribute(
          "href",
          `/organization/${tab}`,
        );
      }
    }
    expect(within(nav).getByRole("link", { name: en.organization.tab.members })).toHaveAttribute("aria-current", "page");
    expect(within(nav).queryByRole("button", { name: en.nav.projects })).toBeNull();
    expect(within(nav).getByRole("link", { name: en.organization.nav.back })).toHaveAttribute(
      "href",
      "/projects/helsinki/home",
    );
    const header = screen.getByRole("banner");
    const links = await within(header).findAllByRole("link", { name: en.nav.organization });
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute("aria-current", "page");
  });

  // UI-75, UI-87 (T-2879): who does not administer the organization meets no entry to its page
  // and no whole-project export anywhere in the top bar.
  it("shows no Administration entry and no project export to anybody but an administrator", async () => {
    renderAt("/projects/helsinki/spaces", {
      project: "helsinki",
      bootstrap: false,
      grants: [{ role: "viewer", binding: "everyone", rule: { kinds: ["ContextSpace", "Project"], verbs: ["read"] } }],
    });
    const header = await screen.findByRole("banner");
    await screen.findByRole("navigation", { name: "Main navigation" });
    // Hidden while the permissions are read is no proof: wait for the organization's answer.
    await waitFor(() =>
      expect(
        vi.mocked(fetch).mock.calls.some(([input]) =>
          String(input instanceof Request ? input.url : input).includes("/projects/org/permissions/me"),
        ),
      ).toBe(true),
    );
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(within(header).queryByRole("link", { name: en.nav.organization })).toBeNull();
    expect(within(header).queryByRole("button", { name: /Export project/i })).toBeNull();
  });

  // V04, T-4831: who does not administer the organization meets no section link in its sidebar.
  it("gives somebody who does not administer only the way back", async () => {
    renderAt("/organization/members", {
      project: "org",
      bootstrap: false,
      grants: [{ role: "viewer", binding: "everyone", rule: { kinds: ["Project"], verbs: ["read"] } }],
    });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    expect(within(nav).getByRole("link", { name: en.organization.nav.back })).toBeInTheDocument();
    for (const tab of ORGANIZATION_TABS) {
      expect(within(nav).queryByRole("link", { name: en.organization.tab[tab] })).toBeNull();
    }
  });

  // T-4831: every section is reachable from the side navigation, and none is a missing page.
  it("opens each section from the side navigation at its own address", async () => {
    const user = userEvent.setup();
    renderAt("/organization/settings");
    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    for (const tab of ORGANIZATION_TABS) {
      await user.click(await within(nav).findByRole("link", { name: en.organization.tab[tab] }));
      await waitFor(() => expect(window.location.pathname).toBe(`/organization/${tab}`));
      expect(within(nav).getByRole("link", { name: en.organization.tab[tab] })).toHaveAttribute("aria-current", "page");
      expect(screen.queryByText(en.app.notFound.title)).toBeNull();
    }
  });

  it("keeps the signed-in person in the header only, not a second copy at the sidebar's foot (owner 2026-11-01)", async () => {
    renderAt("/organization/members");
    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    expect(within(nav).queryByRole("region", { name: en.nav.profile })).toBeNull();
    expect(within(nav).queryByRole("button", { name: en.auth.signOut })).toBeNull();
  });

  it("opens a tab's create form as a page of its own, the assistant's `…/new` address", async () => {
    renderAt("/organization/groups/new");
    expect(await screen.findByRole("heading", { name: en.access.groups.newTitle })).toBeInTheDocument();
  });

  // T-2750: the organization's Members and Roles answered their `…/new` with "nothing called
  // rolebindings/roles here".
  it("opens Members' grant form at /organization/members/new", async () => {
    // Only a person who reads the organization's bindings sees Members at all (PF-59).
    renderAt("/organization/members/new", { project: "org", bootstrap: true, grants: [] });
    expect(await screen.findByRole("heading", { name: en.access.roles.grantTitle })).toBeInTheDocument();
    expect(screen.queryByText(/rolebindings/)).toBeNull();
  });

  it("opens Roles' new-role form at /organization/roles/new", async () => {
    renderAt("/organization/roles/new");
    expect(await screen.findByRole("heading", { name: en.access.projectRoles.newTitle })).toBeInTheDocument();
  });

  it("answers a tab address with something that is no form with the not-found page", async () => {
    renderAt("/organization/groups/whatever/else");
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
  });

  it("answers an address that names no tab with the not-found page", async () => {
    renderAt("/organization/billing");
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
  });
});
