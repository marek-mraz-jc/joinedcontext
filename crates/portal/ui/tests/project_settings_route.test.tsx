/**
 * T-2606, UI-76, Architecture/09 §14.3: the addresses of Project settings.
 *
 * `/projects/{project}/access` is permanent and client-side: it lands on Settings → Members with
 * the query string it came with, so a saved link, a chat message and the assistant's older
 * `?grant=` answers still open what they named. `/projects/{project}/settings` opens General, a
 * tab's form is a page of its own under the tab (`…/new`, `…/{name}/edit`), an address that names
 * no tab is the Portal's not-found page, and the project's menu marks Project settings current on
 * every tab.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { settingsSearch } from "../src/router";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["portal-viewer"] };
const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] };
const BOOTSTRAP = { project: "helsinki", bootstrap: true, grants: [] };

function renderAt(path: string) {
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
            ? BOOTSTRAP
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

describe("the addresses of Project settings (T-2606)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends an old Access link to Members, with its query string", async () => {
    renderAt("/projects/helsinki/access?grant=jana-kovacova-steward-helsinki");
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/settings/members"));
    expect(window.location.search).toBe("?grant=jana-kovacova-steward-helsinki");
  });

  it("opens a bare /projects/{project} on the project's home (T-5015)", async () => {
    renderAt("/projects/helsinki");
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/home"));
  });

  it("opens /projects/{project}/settings on General", async () => {
    renderAt("/projects/helsinki/settings");
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/settings/general"));
    expect(await screen.findByRole("tab", { name: en.projectSettings.tab.general })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("marks Project settings current in the project's menu on every tab", async () => {
    renderAt("/projects/helsinki/settings/roles");
    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    const link = await within(nav).findByRole("link", { name: en.nav.settings });
    expect(link).toHaveAttribute("aria-current", "page");
  });

  it("opens a tab's create form as a page of its own, the assistant's `…/new` address", async () => {
    renderAt("/projects/helsinki/settings/service-accounts/new");
    expect(await screen.findByRole("heading", { name: en.access.accounts.add })).toBeInTheDocument();
  });

  // T-2750: Members and Roles kept the create dialog in their own state, so the address the
  // assistant and a shared link use said "there is nothing called rolebindings here".
  it("opens Members' grant form at settings/members/new", async () => {
    renderAt("/projects/helsinki/settings/members/new");
    expect(await screen.findByRole("heading", { name: en.access.roles.grantTitle })).toBeInTheDocument();
    expect(screen.queryByText(/rolebindings/)).toBeNull();
  });

  it("opens Roles' new-role form at settings/roles/new", async () => {
    renderAt("/projects/helsinki/settings/roles/new");
    expect(await screen.findByRole("heading", { name: en.access.projectRoles.newTitle })).toBeInTheDocument();
  });

  it("answers an address that names no tab, or no form, with the not-found page", async () => {
    renderAt("/projects/helsinki/settings/billing");
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
  });

  it("answers a tab address with something that is no form with the not-found page", async () => {
    renderAt("/projects/helsinki/settings/members/whatever/else");
    expect(await screen.findByText(en.app.notFound.title)).toBeInTheDocument();
  });

  it("keeps ?endpoint= only as a resource name (T-5593)", () => {
    expect(settingsSearch({ endpoint: "air-quality" })).toEqual({ endpoint: "air-quality" });
    for (const bad of ["Air", "../x", "a/b", "", 7, ["air"], "x".repeat(64)]) {
      expect(settingsSearch({ endpoint: bad })).toEqual({ endpoint: undefined });
    }
  });
});
