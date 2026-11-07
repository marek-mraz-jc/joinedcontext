import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const EMPTY_LIST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [],
};

/** What the dev repository holds: two `projects/<slug>/` directories. */
const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  // PF-109: banskabystrica is the sample of real open data.
  items: [{ name: "helsinki", sample: false }, { name: "banskabystrica", sample: true }],
};

describe("portal shell", () => {
  let client: QueryClient;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/");
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? PROJECTS
          : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    // The index route shows a bare page until the project list is in; the shell is up once
    // its sidebar is.
    await waitFor(() => {
      expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("exposes the ARIA landmarks a keyboard user navigates by", () => {
    expect(screen.getByRole("banner")).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    expect(screen.getByRole("main")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#main");
  });

  it("lists every section of the resource API in the sidebar", async () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    for (const label of [
      "Context Spaces",
      "Endpoints",
      "Pipelines",
      "Dashboards",
      "Applications",
      "Approvals",
      // Project → Access folded into Project settings (T-2606).
      "Project settings",
    ]) {
      expect(within(nav).getByRole("link", { name: label })).toBeInTheDocument();
    }
    // Administration is one button in the header, not a sidebar entry (owner, 2026-09-24,
    // 2026-09-25), shown once the permissions say the person administers the organization.
    expect(within(nav).queryByRole("link", { name: "Administration" })).toBeNull();
    expect(
      await within(screen.getByRole("banner")).findByRole("link", { name: "Administration" }),
    ).toHaveAttribute("href", "/organization/settings");
  });

  it("marks the section the router is on with aria-current", async () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    // "/" lands on the project's home (T-3233).
    expect(within(nav).getByRole("link", { name: "Home" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await userEvent.click(within(nav).getByRole("link", { name: "Endpoints" }));

    await waitFor(() => {
      expect(within(nav).getByRole("link", { name: "Endpoints" })).toHaveAttribute(
        "aria-current",
        "page",
      );
    });
    expect(within(nav).getByRole("link", { name: "Home" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("navigating a section asks the API for that plural", async () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    await userEvent.click(within(nav).getByRole("link", { name: "Dashboards" }));

    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as RequestInfo | URL;
        return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      });
      expect(urls.some((url) => url.includes("/projects/helsinki/dashboards"))).toBe(true);
    });
  });

  // F-UI1-002: the trail decodes each segment of the address; a broken escape keeps the segment
  // as typed instead of throwing above every error boundary and blanking the Portal.
  it("keeps the shell up on an address with a broken escape", async () => {
    window.history.pushState({}, "", "/projects/helsinki/spaces/%E0%A4%A");
    window.dispatchEvent(new PopStateEvent("popstate"));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/helsinki/spaces/%E0%A4%A");
      expect(screen.getByRole("navigation", { name: "Breadcrumb" })).toBeInTheDocument();
    });
    expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
  });

  it("shows the project and the active section as a breadcrumb trail", async () => {
    // A route change remounts the shell, so the breadcrumb node has to be looked up again.
    const crumbs = () => screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs()).getByRole("link", { name: "helsinki" })).toBeInTheDocument();
    expect(within(crumbs()).getByText("Home")).toHaveAttribute("aria-current", "page");

    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    await userEvent.click(within(nav).getByRole("link", { name: "Approvals" }));

    await waitFor(() => {
      expect(within(crumbs()).getByText("Approvals")).toHaveAttribute("aria-current", "page");
    });
    expect(within(crumbs()).queryByText("Home")).not.toBeInTheDocument();
  });

  it("names a static page under Context Spaces in words, not as a space called 'complete'", async () => {
    window.history.pushState({}, "", "/projects/helsinki/spaces/complete");
    window.dispatchEvent(new PopStateEvent("popstate"));
    const crumbs = () => screen.getByRole("navigation", { name: "Breadcrumb" });
    await waitFor(() => {
      expect(within(crumbs()).getByText(i18n.t("spaces.complete.title"))).toHaveAttribute("aria-current", "page");
    });
    expect(within(crumbs()).queryByText("complete")).toBeNull();
    expect(within(crumbs()).getByRole("link", { name: i18n.t("nav.spaces") })).toBeInTheDocument();
  });

  // T-4216 (photo audit T-4157, T-4163): the pages below the section list end the trail with
  // their own name, like every section does, not at the project.
  it("ends the trail with the page's own name on the pages below the section list", async () => {
    const crumbs = () => screen.getByRole("navigation", { name: "Breadcrumb" });
    // A route change remounts the shell: the navigation is looked up again for each page.
    const nav = () => screen.getByRole("navigation", { name: "Main navigation" });
    for (const label of [i18n.t("nav.models"), i18n.t("nav.explore")]) {
      await userEvent.click(within(nav()).getByRole("link", { name: label }));
      await waitFor(() => {
        expect(within(crumbs()).getByText(label)).toHaveAttribute("aria-current", "page");
      });
    }
  });

  // T-6090 (owner walk): a model defines what every space holds, so Data models sits right under
  // Context Spaces, not far down after Policies and Project settings.
  it("lists Data models right after Context Spaces", () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    const names = within(nav).getAllByRole("link").map((link) => link.textContent?.trim());
    const spaces = names.indexOf(i18n.t("nav.spaces"));
    expect(spaces).toBeGreaterThan(-1);
    expect(names[spaces + 1]).toBe(i18n.t("nav.models"));
    expect(names.filter((name) => name === i18n.t("nav.models"))).toHaveLength(1);
  });

  // T-4223 (photo audit T-4157): Administration is the organization's page, not a project's;
  // its trail named the project the person came from.
  it("trails the Administration page from the organization, without a project", async () => {
    const admin = await within(screen.getByRole("banner")).findByRole("link", { name: "Administration" });
    await userEvent.click(admin);
    await waitFor(() => {
      const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
      expect(within(crumbs).getByText("Administration")).toHaveAttribute("aria-current", "page");
    });
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).queryByRole("link", { name: "helsinki" })).toBeNull();
  });

  // T-2760: the floating assistant button covered a table's last column with nothing to scroll.
  it("leaves room under the page for the floating assistant button", () => {
    const page = screen.getByRole("main").firstElementChild as HTMLElement;
    expect(page.className).toMatch(/(^|\s)pb-24(\s|$)/);
  });

  it("offers the active project in a switcher and the identity in a user menu", () => {
    expect(screen.getByRole("button", { name: "Projects" })).toHaveTextContent("helsinki");
    expect(
      screen.getByRole("button", { name: "Signed in as Jana Kováčová" }),
    ).toBeInTheDocument();
  });

  it("lists every project the API holds and switches to the one picked", async () => {
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Projects" }));

    const items = await screen.findAllByRole("menuitem");
    // Every row the same, no sample badge (owner 2026-10-30).
    expect(items.map((item) => item.textContent?.trim())).toEqual(["helsinki", "banskabystrica"]);

    await user.click(screen.getByRole("menuitem", { name: "banskabystrica" }));

    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/banskabystrica/home");
    });
    expect(screen.getByRole("button", { name: "Projects" })).toHaveTextContent("banskabystrica");
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getByRole("link", { name: "banskabystrica" })).toBeInTheDocument();
    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as RequestInfo | URL;
        return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      });
      // The home asks what the person may do in the project it switched to (T-3233).
      expect(urls.some((url) => url.includes("/projects/banskabystrica/permissions/me"))).toBe(true);
    });
  });

  // PF-61, T-2877: every endpoint of every project is the Organization page's, for
  // administrators; the project menu offers no entry for it to anyone.
  it("offers no entry for every endpoint of every project in the project menu", () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    expect(within(nav).queryByRole("link", { name: "All endpoints" })).not.toBeInTheDocument();
    expect(nav.querySelector('a[href="/endpoints"]')).toBeNull();
  });
});

describe("the Approvals badge", () => {
  const change = (name: string, phase: string, email: string, kind = "Endpoint") => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name, namespace: "banskabystrica" },
    summary: { key: "change.summary.update", params: { kind, name } },
    author: { name: email, email },
    createdAt: "2026-03-03T12:00:00Z",
    status: { lane: "yellow", phase, plan: { update: 1 } },
  });

  function renderShell(changes: unknown[] | null, grants: unknown[]) {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        const [status, body] = url.includes("/auth/me")
          ? [200, { ...IDENTITY, email: "jana@banskabystrica.sk" }]
          : url.includes("/permissions/me")
            ? [200, { project: "banskabystrica", bootstrap: false, grants }]
            : url.includes("/changes")
              ? changes === null
                ? [403, { title: "Forbidden", status: 403 }]
                : [200, { ...EMPTY_LIST, items: changes }]
              : url.endsWith("/api/v1/projects")
                ? [200, PROJECTS]
                : [200, EMPTY_LIST];
        return Promise.resolve(
          new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
        );
      }),
    );
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  const APPROVER = [{ role: "approver", binding: "b", rule: { kinds: ["Endpoint"], verbs: ["approve"] } }];

  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("counts the pending changes of others on kinds the person may approve", async () => {
    renderShell(
      [
        change("chg-1", "PendingApproval", "marek@banskabystrica.sk"),
        change("chg-2", "PendingApproval", "eva@banskabystrica.sk"),
        change("chg-3", "Applied", "eva@banskabystrica.sk"),
        change("chg-4", "PendingApproval", "jana@banskabystrica.sk"),
        change("chg-5", "PendingApproval", "eva@banskabystrica.sk", "RoleBinding"),
      ],
      APPROVER,
    );
    const link = await screen.findByRole("link", { name: /Approvals/ });
    await waitFor(() => expect(link).toHaveTextContent("2 changes wait for your approval"));
  });

  it("shows no badge when nothing waits, or for a viewer who may approve nothing", async () => {
    renderShell([change("chg-1", "PendingApproval", "marek@banskabystrica.sk")], []);
    const link = await screen.findByRole("link", { name: /Approvals/ });
    await new Promise((r) => setTimeout(r, 50));
    expect(link).not.toHaveTextContent(/wait/);
  });

  it("shows no badge when the API refuses the list", async () => {
    renderShell(null, APPROVER);
    const link = await screen.findByRole("link", { name: /Approvals/ });
    await new Promise((r) => setTimeout(r, 50));
    expect(link).not.toHaveTextContent(/wait/);
  });
});

// T-2760: a copy's own pages had no trail; they sit under Copies, the copy named last.
describe("the trail of a copy's page", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads project › Copies › the copy, and Copies leads back", async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/workspaces/air-v2/compare");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        const body = url.includes("/auth/me")
          ? IDENTITY
          : url.endsWith("/api/v1/projects")
            ? PROJECTS
            : url.endsWith("/compare")
              ? { files: [], conflicts: [] }
              : EMPTY_LIST;
        return Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
        );
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const crumbs = await screen.findByRole("navigation", { name: "Breadcrumb" });
    await waitFor(() => expect(within(crumbs).getByText("air-v2")).toHaveAttribute("aria-current", "page"));
    expect(within(crumbs).getByRole("link", { name: "Copies" })).toHaveAttribute(
      "href",
      "/projects/helsinki/workspaces",
    );
  });
});

// T-3239, T-3241: every item page reads organization › project › section › item, each a link,
// and an item's bare address opens it, on its form where its kind has no page of its own.
describe("the trail to one item", () => {
  function open(path: string, organisation: string) {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        const body = url.includes("/auth/me")
          ? IDENTITY
          : url.endsWith("/api/v1/projects")
            ? PROJECTS
            : url.endsWith("/api/v1/branding")
              ? { instanceName: "joinedcontext", organisation }
              : EMPTY_LIST;
        return Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
        );
      }),
    );
    window.history.pushState({}, "", path);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads organization › project › Pipelines › the pipeline, each part a link", async () => {
    open("/projects/helsinki/pipelines/air-quality/edit", "City of Helsinki");
    const crumbs = await screen.findByRole("navigation", { name: "Breadcrumb" });
    await waitFor(() => expect(within(crumbs).getByText("City of Helsinki")).toBeInTheDocument());
    expect(within(crumbs).getByRole("link", { name: "helsinki" })).toHaveAttribute("href", "/projects/helsinki/spaces");
    expect(within(crumbs).getByRole("link", { name: "Pipelines" })).toHaveAttribute("href", "/projects/helsinki/pipelines");
    const item = within(crumbs).getByRole("link", { name: "air-quality" });
    expect(item).toHaveAttribute("href", "/projects/helsinki/pipelines/air-quality");
    expect(item).toHaveAttribute("aria-current", "page");
  });

  it("names no organization an installation does not name, and opens an item's bare address on its form", async () => {
    open("/projects/helsinki/datasources/air-feed", "");
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/datasources/air-feed/edit"));
    const crumbs = await screen.findByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getAllByRole("listitem")[0]).toHaveTextContent("helsinki");
  });
});

// T-2908: the App's Open page takes the whole working area. The Shell drops what caps or pads a
// page, and folds the project navigation behind the menu button at every width.
describe("portal shell around a page that fills it", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        const body = url.includes("/auth/me")
          ? IDENTITY
          : url.endsWith("/api/v1/projects")
            ? PROJECTS
            : url.endsWith("/apps/city-bikes")
              ? {
                  apiVersion: "joinedcontext.com/v1alpha1",
                  kind: "App",
                  metadata: { name: "city-bikes", namespace: "helsinki" },
                  spec: { kind: "static", visibility: "internal", lifecycle: "published", dataNeeds: [] },
                  status: { build: { commit: "4f2a9c1e0b7d3a5f6c8e9d0a1b2c3d4e5f6a7b8c" } },
                }
              : EMPTY_LIST;
        return Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
        );
      }),
    );
    window.history.pushState({}, "", "/projects/helsinki/apps/city-bikes/open");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(document.querySelector("iframe")).not.toBeNull(), { timeout: 4000 });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("gives the page the viewport under the header, with no breadcrumb, cap or footer", () => {
    const main = screen.getByRole("main");
    // The Shell is the window's height and does not scroll; the page grows into what the header
    // leaves, which is how the App's frame fills it without a fixed height of its own.
    expect(main.closest(".h-dvh.overflow-hidden")).not.toBeNull();
    expect(main.className).toMatch(/\bflex-1\b/);
    expect(main.className).toMatch(/\bmin-h-0\b/);
    expect(main.querySelector(".max-w-content")).toBeNull();
    expect(screen.queryByRole("navigation", { name: "Breadcrumb" })).toBeNull();
    expect(screen.queryByRole("contentinfo")).toBeNull();
  });

  it("folds the project navigation behind the menu button at every width, and opens it on demand", async () => {
    const user = userEvent.setup();
    const nav = document.getElementById("portal-sidebar") as HTMLElement;
    expect(nav.className).not.toMatch(/\bmd:flex\b/);
    expect(nav.className).toMatch(/\bhidden\b/);
    const menu = screen.getByRole("button", { name: "Menu" });
    expect(menu.className).not.toMatch(/\bmd:hidden\b/);
    await user.click(menu);
    expect(menu).toHaveAttribute("aria-expanded", "true");
    expect(nav.className).toMatch(/\bfixed\b/);
    await user.keyboard("{Escape}");
    expect(menu).toHaveAttribute("aria-expanded", "false");
    expect(menu).toHaveFocus();
  });
});
