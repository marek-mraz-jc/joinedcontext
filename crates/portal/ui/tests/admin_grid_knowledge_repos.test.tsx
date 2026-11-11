// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/knowledge/AssistantContextPreview.tsx,
// src/pages/knowledge/SourcePage.tsx, src/pages/repositories/RepositoriesPage.tsx and
// src/pages/repositories/SettingsView.tsx.
/**
 * T-6749: the knowledge and repository lists on the shared grid. Each is walked through its real
 * route: the rows are what the API returned, the grid sorts and searches them, and an empty list
 * or a failed read says so in the page's words.
 */
import type { ReactElement } from "react";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { AssistantContext } from "../src/pages/knowledge/knowledge";
import { ContextView } from "../src/pages/knowledge/AssistantContextPreview";
import { jsonResponse, problem, renderRoute, VIEWER } from "./pageHarness";

const P = "/projects/helsinki/repositories";
const KNOWLEDGE = "/api/v1/projects/helsinki/knowledge/sources/web/documents";

const GRANTS = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["Project"], verbs: ["read", "propose", "approve"] } }],
};

const repos = (items: unknown[]) => (path: string): Response | undefined =>
  path.endsWith("/ops/jc_repository_list") ? jsonResponse({ items, total: items.length }) : undefined;

const REPOSITORIES = [
  { name: "zeta", role: "application", defaultBranch: "main", sizeBytes: 5000 },
  { name: "alpha", role: "configuration", defaultBranch: "main", sizeBytes: 90000, description: "The config" },
  { name: "mid", role: "application", defaultBranch: "dev", sizeBytes: 100, archived: true },
];

const names = () =>
  [...document.querySelectorAll(".jc-data-grid tbody tr")].map((tr) => tr.querySelector("th")?.querySelector("a, span")?.textContent ?? "");

beforeEach(async () => {
  await i18n.changeLanguage("en");
  try {
    window.localStorage.clear();
  } catch {
    // no storage in this run
  }
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("repositories list on the grid", () => {
  it("sorts by size from the header menu and keeps the server filter box", async () => {
    await renderRoute({ path: P, permissions: GRANTS, answer: repos(REPOSITORIES) });
    await screen.findByRole("link", { name: "alpha" });
    expect(names()).toHaveLength(3);
    fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.repositories.column.size}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAscNumber }));
    expect(names()).toEqual(["mid", "zeta", "alpha"]);
    expect(screen.getAllByRole("searchbox")).toHaveLength(1);
  });

  it("says what lives here when there are none", async () => {
    await renderRoute({ path: P, permissions: GRANTS, answer: repos([]) });
    expect(await screen.findByText(en.repositories.empty.title)).toBeInTheDocument();
  });

  it("names a refusal instead of an empty grid", async () => {
    await renderRoute({
      path: P,
      permissions: GRANTS,
      answer: (path) => (path.endsWith("/ops/jc_repository_list") ? problem(503, "The forge did not answer.") : undefined),
    });
    expect(await screen.findByText(/The forge did not answer/)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });
});

describe("repository settings on the grid", () => {
  const settings = {
    repository: "alpha",
    protection: [
      { pattern: "release/*", pushAllowed: true, forcePushAllowed: false, requiredApprovals: 2 },
      { pattern: "main", pushAllowed: false, forcePushAllowed: false, requiredApprovals: 1 },
    ],
    webhooks: [],
    collaborators: [],
    packages: [
      { name: "b", version: "1.0", type: "container", digest: "sha256:abcdef0123456789", size: 10, createdAt: "2026-01-02T00:00:00Z" },
      { name: "a", version: "2.0", type: "npm", digest: "sha256:0123456789abcdef", size: 20, createdAt: "2026-02-02T00:00:00Z" },
    ],
  };
  const answer = (path: string): Response | undefined => {
    if (path.endsWith("/ops/jc_repository_settings")) return jsonResponse(settings);
    if (path.endsWith("/ops/jc_repository_get")) {
      return jsonResponse({ name: "alpha", owner: "helsinki", role: "configuration", defaultBranch: "main", sizeBytes: 1, cloneUrl: "https://f/x.git", empty: false });
    }
    return undefined;
  };

  it("sorts and searches the protection rules and the packages", async () => {
    await renderRoute({ path: `${P}/alpha?tab=settings`, permissions: GRANTS, answer });
    const rules = await screen.findByRole("table", { name: en.repositories.settings.protection });
    fireEvent.click(within(rules).getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.repositories.settings.pattern}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAsc }));
    expect([...rules.querySelectorAll("tbody th")].map((th) => th.textContent)).toEqual(["main", "release/*"]);

    const packages = screen.getByRole("table", { name: en.repositories.settings.packages });
    expect(within(packages).getAllByRole("row")).toHaveLength(3);
    fireEvent.change(screen.getAllByRole("searchbox", { name: en.dataGrid.searchRows })[1], { target: { value: "npm" } });
    expect(within(packages).getAllByRole("row")).toHaveLength(2);
  });
});

describe("the assistant's context preview on the grid", () => {
  const context: AssistantContext = {
    publicId: "p",
    channel: "public",
    title: null,
    languages: ["en"],
    limits: { perClientPerHour: null, perClientPerDay: null, messagesPerConversation: null, questionChars: null },
    problems: [],
    categories: [],
    omitted: [],
    sources: [],
    endpoints: [
      { slug: "b", name: "b", title: "Buses", project: "city", audience: null, timeoutSeconds: 30, themes: [{ code: "t", label: "Transport" }], types: ["Bus"], tools: [] },
      { slug: "a", name: "a", title: "Air", project: "env", audience: null, timeoutSeconds: 30, themes: [], types: [], tools: ["query_entities"] },
    ],
  };
  const inProviders = (node: ReactElement) =>
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
      </QueryClientProvider>,
    );

  it("sorts and searches the Endpoints the dry run named", () => {
    inProviders(<ContextView context={context} />);
    expect(screen.getAllByRole("rowheader").map((th) => th.textContent)).toEqual(["Buses", "Air"]);
    fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.knowledge.context.endpoint}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAsc }));
    expect(screen.getAllByRole("rowheader").map((th) => th.textContent)).toEqual(["Air", "Buses"]);
    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "zzz" } });
    expect(screen.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
  });

  it("says so when no Endpoint is in reach", () => {
    inProviders(<ContextView context={{ ...context, endpoints: [] }} />);
    expect(screen.getByText(en.knowledge.context.noEndpoints)).toBeInTheDocument();
  });
});

describe("a source's documents on the grid", () => {
  const document_ = (id: number, url: string, bytes: number) => ({
    id, url, pageId: null, mime: "application/pdf", bytes, pages: 3, offDomain: false, status: "fetched",
    included: true, excludedBy: null, passages: 4, title: null, modified: "2026-01-15T00:00:00Z",
  });
  const answer = (items: unknown[] | null) => (path: string): Response | undefined => {
    if (path === "/api/v1/projects/helsinki/knowledge/sources") {
      return jsonResponse({
        items: [{ source: "web", type: "website", state: "crawled", startUrls: ["https://x.org/"], ckanInstanceRef: null, schedule: null, visibility: "public", pages: 1, pagesIncluded: 1, documents: 2, passages: 8, embedded: 8, pending: 0, failed: 0, languages: {}, job: { state: "done", attempts: 1, error: null } }],
      });
    }
    if (path === KNOWLEDGE) return items ? jsonResponse({ items }) : problem(503, "The index did not answer.");
    return undefined;
  };
  const open = (items: unknown[] | null) =>
    renderRoute({ path: "/projects/helsinki/knowledge/web", permissions: VIEWER, answer: answer(items) }).then(async (page) => {
      fireEvent.click(await screen.findByRole("tab", { name: "Documents" }));
      return page;
    });

  it("sorts by size from the header menu", async () => {
    await open([document_(1, "https://x.org/big.pdf", 9000), document_(2, "https://x.org/small.pdf", 10)]);
    await screen.findByRole("link", { name: /big\.pdf/ });
    const table = screen.getByRole("table", { name: en.knowledge.source.documents });
    fireEvent.click(within(table).getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.knowledge.column.size}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAscNumber }));
    const links = [...table.querySelectorAll("tbody th a")].map((a) => a.getAttribute("href"));
    expect(links).toEqual(["https://x.org/small.pdf", "https://x.org/big.pdf"]);
  });

  it("says there are no documents, and shows the reason when the read fails", async () => {
    await open([]);
    expect(await screen.findByText(en.knowledge.source.noDocuments)).toBeInTheDocument();
    cleanup();
    await open(null);
    expect(await screen.findByText(/The index did not answer/)).toBeInTheDocument();
  });
});
