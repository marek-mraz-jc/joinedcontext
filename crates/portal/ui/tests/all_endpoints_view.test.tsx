import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const LIST = "joinedcontext.com/v1alpha1";

function endpoint(project: string, space: string, name: string, slug: string, audience: string) {
  return {
    apiVersion: LIST,
    kind: "Endpoint",
    metadata: { name, namespace: project, labels: { "joinedcontext.com/space": space } },
    spec: { contextSpaceRef: space, slug, audience, enabledRepresentations: ["ngsi-ld", "csv"] },
    status: { phase: "Live" },
  };
}

/** An administrator of the organization (PF-03): the one person the list is for (PF-61, T-2877). */
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding", "Endpoint"], verbs: ["read", "approve", "delete"] } }],
};

/**
 * The dev repository as the organization-level route answers an administrator (PF-61): every
 * Endpoint of every project, in one list, each carrying the project it lives in.
 */
const BY_PATH: Record<string, unknown> = {
  "/api/v1/projects/org/permissions/me": ADMIN,
  "/api/v1/projects": { apiVersion: LIST, kind: "List", items: [{ name: "banskabystrica" }, { name: "helsinki" }] },
  "/api/v1/endpoints": {
    apiVersion: LIST,
    kind: "List",
    items: [
      {
        ...endpoint("banskabystrica", "ovzdusie", "public-air", "k7m2qz4tv6xh3n5jb2ryd3wcfa", "public"),
        spec: {
          ...endpoint("banskabystrica", "ovzdusie", "public-air", "k7m2qz4tv6xh3n5jb2ryd3wcfa", "public").spec,
          publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "bb-open-data" }, name: "ovzdusie-bb" } },
        },
      },
      endpoint("helsinki", "helsinki", "helsinki-bikes", "a1b2c3d4e5f6g7h8j9k0m1n2p3", "public"),
      endpoint("helsinki", "helsinki", "helsinki-events", "z9y8x7w6v5u4t3s2r1q0p9n8m7", "organization"),
    ],
  },
  // The catalogue the published Endpoint names: its link is this instance's own site (T-3018).
  "/api/v1/projects/banskabystrica/ckaninstances": {
    apiVersion: LIST,
    kind: "List",
    items: [
      {
        apiVersion: LIST,
        kind: "CkanInstance",
        metadata: { name: "bb-open-data", namespace: "banskabystrica" },
        spec: { url: "https://data.city.example", secretRef: { name: "ckan-token" } },
      },
    ],
  },
};

describe("all endpoints view", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  let responses: Record<string, unknown>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/endpoints");
    responses = { ...BY_PATH };
  });

  function renderAt() {
    fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      const body = url.pathname.includes("/auth/me")
        ? IDENTITY
        : (responses[url.pathname] ?? { apiVersion: LIST, kind: "List", items: [] });
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    });
    vi.stubGlobal("fetch", fetchMock);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every endpoint of every project in one-line rows with a way inside the space", async () => {
    renderAt();
    const table = await screen.findByRole("table", { name: "All endpoints" });
    // The old address opens the Organization page's Endpoints tab (T-2877).
    expect(window.location.pathname).toBe("/organization/endpoints");
    await waitFor(() => {
      expect(within(table).getAllByRole("row")).toHaveLength(4);
    });

    const bb = within(table).getByText("public-air").closest("tr") as HTMLTableRowElement;
    expect(within(bb).getByRole("link", { name: "banskabystrica" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/endpoints",
    );
    expect(within(bb).getByRole("link", { name: "Look inside: banskabystrica/ovzdusie" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/spaces/ovzdusie",
    );
    expect(within(bb).getByText("Public")).toBeInTheDocument();
    // One line a row (T-4989): the surfaces as one summary and the row opening the endpoint's
    // page, where each surface link and the catalogue entry are.
    expect(within(bb).getByText("NGSI-LD, MCP, CSV")).toBeInTheDocument();
    expect(within(bb).queryByRole("link", { name: "csv" })).not.toBeInTheDocument();
    expect(within(bb).queryByRole("link", { name: "catalogue" })).not.toBeInTheDocument();
    expect(bb.querySelector("a[data-row-link]")).toHaveAttribute("href", "/projects/banskabystrica/endpoints/public-air");

    const hel = within(table).getByText("helsinki-events").closest("tr") as HTMLTableRowElement;
    expect(within(hel).getByText("Organization")).toBeInTheDocument();
    expect(within(table).getByText("helsinki-bikes")).toBeInTheDocument();

    // One request, answered by the route that decides what is in it — not one per project.
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).toContain("/api/v1/endpoints");
    // No fan-out across projects any more: helsinki's own list is never fetched for this page.
    expect(paths).not.toContain("/api/v1/projects/helsinki/endpoints");
    // The list asks no project for its catalogues: the entry is on the endpoint's page (T-4989).
    expect(paths).not.toContain("/api/v1/projects/banskabystrica/ckaninstances");
  });

  // PF-61, PF-107 (T-3046): a public Endpoint of any project carries the project page's own
  // actions, a non-public one none; the catalogues are asked for only when the administrator
  // opens their list, and each opens onto its Access panel.
  it("offers edit and delete on a public endpoint of any project, and the catalogues' access when asked", async () => {
    const MAY = { project: "x", bootstrap: false, grants: [{ rule: { kinds: ["Endpoint"], verbs: ["read", "propose", "delete"] } }] };
    responses["/api/v1/projects/banskabystrica/permissions/me"] = MAY;
    responses["/api/v1/projects/helsinki/permissions/me"] = MAY;
    renderAt();
    const table = await screen.findByRole("table", { name: "All endpoints" });
    await waitFor(() => {
      expect(within(table).getAllByRole("row")).toHaveLength(4);
    });
    // One menu a row keeps the table to its width (T-4989); it holds Edit and Remove.
    const bikes = within(table).getByText("helsinki-bikes").closest("tr") as HTMLTableRowElement;
    await userEvent.click(within(bikes).getByRole("button", { name: "More actions for helsinki/helsinki-bikes" }));
    expect(await screen.findByRole("menuitem", { name: "Edit" })).not.toHaveAttribute("aria-disabled");
    await userEvent.click(screen.getByRole("menuitem", { name: "Remove" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("helsinki/helsinki-bikes");
    await userEvent.keyboard("{Escape}");
    const events = within(table).getByText("helsinki-events").closest("tr") as HTMLTableRowElement;
    expect(within(events).queryByRole("button", { name: /More actions/ })).toBeNull();

    const show = screen.getByRole("button", { name: en.allEndpoints.ckan.show });
    expect(show).toHaveAttribute("aria-expanded", "false");
    show.click();
    const instance = await screen.findByText("banskabystrica / bb-open-data");
    expect(screen.getByRole("button", { name: en.allEndpoints.ckan.hide })).toHaveAttribute("aria-expanded", "true");
    (instance.closest("details") as HTMLDetailsElement).open = true;
    instance.closest("details")?.dispatchEvent(new Event("toggle"));
    expect(await screen.findByRole("heading", { name: "Access to bb-open-data" })).toBeInTheDocument();
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).toContain("/api/v1/projects/helsinki/ckaninstances");
  });

  // PF-61, T-2877, UI-75: anyone but an administrator is told the page is for administrators,
  // and the list is never asked for.
  it("tells a person who does not administer the organization whose page it is, asking for no list", async () => {
    responses["/api/v1/projects/org/permissions/me"] = {
      project: "org",
      bootstrap: false,
      grants: [{ role: "viewer", binding: "everyone", rule: { kinds: ["Endpoint"], verbs: ["read"] } }],
    };
    renderAt();
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "All endpoints" })).not.toBeInTheDocument();
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).not.toContain("/api/v1/endpoints");
  });

  it("says so when the organization publishes no endpoint anywhere", async () => {
    responses["/api/v1/endpoints"] = { apiVersion: LIST, kind: "List", items: [] };
    renderAt();
    expect(await screen.findByText("No project publishes an endpoint yet.")).toBeInTheDocument();
  });
});
