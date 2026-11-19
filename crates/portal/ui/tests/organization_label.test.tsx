import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", roles: [] };
const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };

/**
 * What `GET /api/v1/projects` answers a person who reads projects of two Organizations: on the
 * deployment's own address `organizations` is empty, on an Organization's host it holds that
 * Organization alone (T-7151).
 */
const ITEMS = [
  { name: "helsinki", sample: false, organization: "hel" },
  { name: "espoo", sample: false, organization: "hel" },
  { name: "zilina", sample: false, organization: "zilina", title: "Mesto Žilina" },
];
const INSTALLATION = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: ITEMS, organizations: [] };
const ZILINA_HOST = {
  ...INSTALLATION,
  items: ITEMS.filter((item) => item.organization === "zilina"),
  organizations: [{ name: "zilina", title: { sk: "Mesto Žilina", en: "City of Žilina" } }],
};
const BRANDING = { instanceName: "Mesto Žilina", shortName: "Žilina" };

function start(projects: object, path: string) {
  window.history.pushState({}, "", path);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? projects
          : url.endsWith("/api/v1/branding") && projects === ZILINA_HOST
            ? BRANDING
            : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const projectItems = async () => {
  await userEvent.setup().click(screen.getByRole("button", { name: "Projects" }));
  // The menu ends with "New project" (T-7404); only the projects are listed here.
  const items = (await screen.findAllByRole("menuitem"))
    .map((item) => item.textContent?.trim())
    .filter((text) => text !== "New project");
  await userEvent.setup().keyboard("{Escape}");
  return items;
};

/** No control that names, switches or lists Organizations, in the top bar or the sidebar. */
function expectNoOrganizationMenu() {
  expect(screen.queryByRole("button", { name: /organi[sz]ation/i })).not.toBeInTheDocument();
  expect(screen.queryByText(/Choose an organization|for example Mesto/i)).not.toBeInTheDocument();
}

describe("the Organization in the header (T-7151)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("names the host's Organization with no menu and lists its projects", async () => {
    start(ZILINA_HOST, "/projects/zilina/spaces");
    await screen.findByRole("navigation", { name: "Main navigation" });
    await waitFor(() => expect(screen.getByRole("banner")).toHaveTextContent("Žilina"));
    expectNoOrganizationMenu();
    expect(await projectItems()).toEqual(["Mesto Žilinazilina"]);
  });

  it("offers no Organization list on the deployment's own address, whatever ?org= says", async () => {
    start(INSTALLATION, "/projects/helsinki/spaces?org=zilina");
    await screen.findByRole("navigation", { name: "Main navigation" });
    expectNoOrganizationMenu();
    expect(await projectItems()).toEqual(["helsinki", "espoo", "Mesto Žilinazilina"]);
    expect(window.location.pathname).toBe("/projects/helsinki/spaces");
  });

  it("builds no menu even from an answer that lists several Organizations", async () => {
    const several = {
      ...INSTALLATION,
      organizations: [{ name: "hel", title: "City of Helsinki" }, { name: "zilina", title: "Mesto Žilina" }],
    };
    start(several, "/projects/helsinki/spaces");
    await screen.findByRole("navigation", { name: "Main navigation" });
    expectNoOrganizationMenu();
    expect(screen.queryByText("City of Helsinki")).not.toBeInTheDocument();
  });
});
