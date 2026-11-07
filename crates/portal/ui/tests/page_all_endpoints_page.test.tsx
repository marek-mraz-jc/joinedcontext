/**
 * T-1787: "All endpoints" against the page contract (UI-11, UI-15, UI-16, EP-08, EP-44, PF-61).
 *
 * `all_endpoints_view.test.tsx` walks to this page through the whole application and keeps what
 * the organization-level route answers. This one renders it where it lives since T-2877, the
 * Organization page's Endpoints tab for an administrator, and asserts the frame: the H1 above it
 * and its own H2, the four states, a table that reads the same
 * with no rows, one row and five hundred, the keyboard, four languages, axe, and — the reason
 * the survey flagged three lines — that every link on it is built by the Portal and checked
 * before it is a link.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { narrowsAndOrders } from "./grid_checks";
import { AllEndpointsPage, ENDPOINTS_PER_PAGE } from "../src/routes/AllEndpointsPage";
import userEvent from "@testing-library/user-event";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  list,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

function endpoint(index: number, overrides: Record<string, unknown> = {}) {
  const project = `city-${index % 7}`;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name: `air-${index}`,
      namespace: project,
      labels: { "joinedcontext.com/space": "ovzdusie" },
      title: { en: `Air quality ${index}` },
    },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: `k7m2qz4tv6xh3n5jb2ryd3wcf${index}`,
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "csv"],
      ...overrides,
    },
    status: { phase: "Live" },
  };
}

/** An administrator of the organization (PF-03), the one person the tab is for. */
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding", "Endpoint"], verbs: ["read", "approve", "delete"] } }],
};

interface World {
  rows?: number;
  fails?: { status: number; detail: string };
  pending?: boolean;
  items?: unknown[];
}

function renderAll(world: World = {}) {
  const { rows = 2, fails, pending = false, items } = world;
  return renderPage(<OrganizationPage tab="endpoints" anchor="helsinki" />, {
    path: "/organization/endpoints",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (!url.pathname.endsWith("/api/v1/endpoints")) return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json(list(items ?? Array.from({ length: rows }, (_, index) => endpoint(index))));
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("all endpoints", () => {
  // UI-15: the Organization page names itself once, and the table carries its own H2 under it.
  // It is outside a project, so the browser tab carries no project name.
  it("has the organization's one H1 and its own H2, with no project", async () => {
    const { container } = renderAll();
    expect(
      await screen.findByRole("heading", { level: 2, name: en.allEndpoints.title }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: en.organization.title })).toBeInTheDocument();
    expect(screen.getByText(en.allEndpoints.lead)).toBeInTheDocument();
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(`${en.organization.title} · Helsinki Region Context`);
    });
  });

  it("says the table is loading rather than showing it empty", async () => {
    renderAll({ pending: true });
    const table = await screen.findByRole("table", { name: en.allEndpoints.title });
    // The wait is announced once, beside the table, and the table says it is busy.
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
    expect(table).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByText(en.allEndpoints.empty)).not.toBeInTheDocument();
  });

  it("says what lands here when nobody publishes an endpoint", async () => {
    const { container } = renderAll({ rows: 0 });
    expect(await screen.findByText(en.allEndpoints.empty)).toBeInTheDocument();
    expect(screen.getByText(en.allEndpoints.emptyHint)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  // A list that failed is not "nobody publishes anything".
  it("shows the reason the list failed, with a retry", async () => {
    renderAll({ fails: { status: 503, detail: "The endpoint index is not answering." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The endpoint index is not answering.",
    );
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.allEndpoints.empty)).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 2, name: en.allEndpoints.title }),
    ).toBeInTheDocument();
  });

  // UI-44: a refusal is the API's own sentence.
  it("says a refusal in the API's own words", async () => {
    renderAll({ fails: { status: 403, detail: "You may read no project's endpoints." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may read no project's endpoints.",
    );
  });

  it.each([0, 1, 500])("draws %i endpoints without changing how the page reads", async (rows) => {
    const { container } = renderAll({ rows });
    if (rows === 0) {
      expect(await screen.findByText(en.allEndpoints.empty)).toBeInTheDocument();
      expectHeadingOutline(container);
      return;
    }
    const table = await screen.findByRole("table", { name: en.allEndpoints.title });
    await waitFor(() => {
      expect(table).not.toHaveAttribute("aria-busy");
    });
    expect(table.querySelectorAll("tbody tr")).toHaveLength(Math.min(rows, ENDPOINTS_PER_PAGE));
    expectHeadingOutline(container);
  });

  // EP-08, EP-44: one direct link per representation, plus the links every endpoint has.
  // T-4989: a row is one line. What the endpoint serves is one summary, not a link per surface
  // (those are on the endpoint's page), and the row opens the endpoint.
  it("summarises what each endpoint serves and opens its page from the row", async () => {
    renderAll({ rows: 1 });
    expect(await screen.findByText("NGSI-LD, MCP, CSV")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "csv" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: en.endpoints.link.index })).not.toBeInTheDocument();
    const open = screen.getByRole("link", { name: "Air quality 0" });
    expect(open).toHaveAttribute("href", "/projects/city-0/endpoints/air-0");
    expect(open).toHaveAttribute("data-row-link");
    expect(open.className).not.toMatch(/(^|\s)underline(\s|$)/);
    // The Context Space is one link on one line, named by its title for a long name.
    const space = screen.getByRole("link", { name: `${en.spaces.inside.open}: city-0/ovzdusie` });
    expect(space).toHaveAttribute("title", "ovzdusie");
    expect(space).toHaveClass("truncate");
  });

  // PF-50: a name out of a manifest is encoded into the row's link.
  it("cannot be walked out of the endpoint's own page by its name", async () => {
    renderAll({ items: [endpoint(0, {}), { ...endpoint(1), metadata: { ...endpoint(1).metadata, name: "../../admin", title: undefined } }] });
    const link = await screen.findByRole("link", { name: "../../admin" });
    expect(link.getAttribute("href")).toBe("/projects/city-1/endpoints/..%2F..%2Fadmin");
  });

  // EP-24, T-2901: MCP is served by every Endpoint that does not opt out, listed or not; a
  // manifest with `mcp: false` is summarised without it.
  it("names MCP for every endpoint unless it opts out", async () => {
    renderAll({ items: [endpoint(0), endpoint(1, { mcp: false })] });
    expect(await screen.findByText("NGSI-LD, MCP, CSV")).toBeInTheDocument();
    expect(screen.getByText("NGSI-LD, CSV")).toBeInTheDocument();
  });

  // UI-16: every link on the page is reached by keyboard, in the order the table is read, and
  // nothing takes focus on arrival.
  it("is reachable by keyboard in the order the table is read", async () => {
    const { container } = renderAll({ rows: 2 });
    await screen.findAllByText("NGSI-LD, MCP, CSV");
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container, 60);
    const projects = screen.getAllByRole("link", { name: /^city-/ });
    expect(reached.indexOf(projects[0])).toBeGreaterThanOrEqual(0);
    expect(reached.indexOf(projects[0])).toBeLessThan(reached.indexOf(projects[1]));
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderAll({ rows: 0 });
      expect(
        await screen.findByRole("heading", { level: 2, name: i18n.t("allEndpoints.title") }),
        `the title is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(await screen.findByText(i18n.t("allEndpoints.emptyHint"))).toBeInTheDocument();
    });
  });

  // PF-50: a title out of a manifest is read as text.
  it("renders an endpoint title that arrived as markup as text", async () => {
    renderAll({ items: [endpoint(0, {})] });
    cleanup();
    renderPage(<AllEndpointsPage />, {
      path: "/endpoints",
      answer: (url) =>
        url.pathname.endsWith("/api/v1/endpoints")
          ? json(
              list([
                {
                  ...endpoint(0),
                  metadata: { ...endpoint(0).metadata, title: { en: "<img src=x onerror=alert(1)>" } },
                },
              ]),
            )
          : undefined,
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with the table full", async () => {
    const { container } = renderAll({ rows: 4 });
    await screen.findAllByText("NGSI-LD, MCP, CSV");
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderAll({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });

  // T-4831 (D-endpoints, 2026-10-20): all endpoints were one 22,822 px table with no filter.
  it("pages the list ten at a time and finds an endpoint by name or project", async () => {
    const user = userEvent.setup();
    renderAll({ rows: 25 });
    const table = await screen.findByRole("table", { name: en.allEndpoints.title });
    await waitFor(() => expect(table.querySelectorAll("tbody tr")).toHaveLength(ENDPOINTS_PER_PAGE));
    expect(screen.getByText("Showing 10 of 25")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show 10 more" }));
    expect(within(table).getByText("Air quality 19")).toBeInTheDocument();

    // A search goes back to the first page of what it matches: air-2 and air-20..air-24.
    await user.type(screen.getByRole("searchbox", { name: en.allEndpoints.find }), "air-2");
    expect(table.querySelectorAll("tbody tr")).toHaveLength(6);
    await user.type(screen.getByRole("searchbox", { name: en.allEndpoints.find }), "nothing-like-it");
    expect(within(table).getByText(en.grid.noMatch)).toBeInTheDocument();
  });

  // T-6206: the shared grid's search, sort and project filter.
  it("searches, sorts and filters on the shared grid", async () => {
    renderAll({ rows: 3, items: [endpoint(5), endpoint(9, { audience: "organization" }), endpoint(1)] });
    const table = await screen.findByRole("table", { name: en.allEndpoints.title });
    await screen.findByText("Air quality 9");
    await narrowsAndOrders(table, {
      search: { label: en.allEndpoints.find, text: "Air quality 9", kept: ["Air quality 9"] },
      sort: { column: en.allEndpoints.field.project, ascending: ["Air quality 1", "Air quality 9", "Air quality 5"] },
      filter: { value: new RegExp(`^${en.endpoints.audience.organization}`), kept: ["Air quality 9"] },
    });
  });
});
