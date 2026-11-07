/**
 * A section the installation hides (T-2874): Dashboards while `JC_PORTAL_DASHBOARDS` is not
 * `true`. Hidden, it is out of the menu, its addresses go to the project's spaces, and the
 * assistant does not offer its path; shown, all of it is back untouched.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { isHiddenSection } from "../src/components/layout/navigation";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-approver"] };
const LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List" };

function renderAt(path: string, hiddenSections: string[]) {
  window.history.pushState({}, "", path);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url, "http://localhost");
      const body = url.pathname.endsWith("/auth/me")
        ? IDENTITY
        : url.pathname === "/api/v1/branding"
          ? { instanceName: "joinedcontext", hiddenSections }
          : url.pathname === "/api/v1/projects"
            ? { ...LIST, items: [{ name: "helsinki" }] }
            : { ...LIST, items: [] };
      return Response.json(body);
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

async function menu(): Promise<HTMLElement> {
  return screen.findByRole("navigation", { name: "Main navigation" });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a hidden section", () => {
  it("is hidden while the branding has not answered, then only when it says so", () => {
    expect(isHiddenSection("dashboards", undefined)).toBe(true);
    expect(isHiddenSection("spaces", undefined)).toBe(false);
    expect(isHiddenSection("dashboards", ["dashboards"])).toBe(true);
    expect(isHiddenSection("dashboards", [])).toBe(false);
  });

  it("is out of the menu, and every other section stays", async () => {
    renderAt("/projects/helsinki/spaces", ["dashboards"]);
    const nav = await menu();
    await waitFor(() => expect(within(nav).getByRole("link", { name: en.nav.spaces })).toBeInTheDocument());
    await waitFor(() => expect(within(nav).queryByRole("link", { name: en.nav.dashboards })).toBeNull());
    expect(within(nav).getByRole("link", { name: en.nav.apps })).toBeInTheDocument();
  });

  it.each(["/projects/helsinki/dashboards", "/projects/helsinki/dashboards/new"])(
    "sends the bookmarked address %s to the project's spaces",
    async (path) => {
      renderAt(path, ["dashboards"]);
      await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces"));
      expect(screen.queryByRole("heading", { name: en.nav.dashboards })).toBeNull();
    },
  );

  it("is back, menu and page, when the installation shows it", async () => {
    renderAt("/projects/helsinki/dashboards", []);
    const nav = await menu();
    expect(await within(nav).findByRole("link", { name: en.nav.dashboards })).toBeInTheDocument();
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/dashboards"));
  });
});
