// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/routes/ApprovalsPage.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import userEvent from "@testing-library/user-event";
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { ApprovalsPage } from "../src/routes/ApprovalsPage";
import { AuthProvider } from "../src/auth/AuthProvider";
import { expectNoAxeViolations, json, problem, renderPage } from "./page_contract";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

function proposal(over: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
    summary: {
      key: "change.summary.update",
      params: { kind: "Endpoint", name: "air-quality", fields: 2 },
    },
    author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
    createdAt: "2026-03-03T12:00:00Z",
    status: {
      lane: "yellow",
      phase: "PendingApproval",
      plan: { update: 1 },
      mergeRequest: "https://gitea.example/city/city-config/pulls/7",
    },
    planFields: [{ path: "spec.audience", from: "public", to: "internal" }],
    ...over,
  };
}

const CHANGES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeList",
  items: [
    proposal(),
    proposal({
      metadata: { name: "chg-9f8e7d6c", namespace: "banskabystrica" },
      summary: {
        key: "change.summary.delete",
        params: { kind: "ContextSpace", name: "doprava", fields: 0 },
      },
      author: { name: "Peter Novák", email: "peter@banskabystrica.sk" },
      createdAt: "2026-03-04T12:00:00Z",
      status: { lane: "red", phase: "PendingApproval", plan: { delete: 1 } },
    }),
  ],
};

function renderApprovals(body: unknown = CHANGES) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const payload = url.includes("/auth/me") ? IDENTITY : body;
    return Promise.resolve(
      new Response(JSON.stringify(payload), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("pending approvals view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/approvals");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("asks the changes endpoint of the project in the URL", async () => {
    const fetchMock = renderApprovals();
    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as RequestInfo | URL;
        return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      });
      expect(urls.some((url) => url.endsWith("/api/v1/projects/banskabystrica/changes"))).toBe(
        true,
      );
    });
  });

  it("summarises each proposal in words, not in field paths", async () => {
    renderApprovals();
    expect(
      await screen.findByRole("link", { name: 'Update Endpoint "air-quality" (2 fields changed)' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: 'Remove ContextSpace "doprava"' }),
    ).toBeInTheDocument();
  });

  it("shows the risk lane of every proposal as a chip", async () => {
    renderApprovals();
    await screen.findByText(en.lane.yellow);
    expect(screen.getByText(en.lane.yellow)).toHaveAttribute("title", en.lane.yellowHelp);
    expect(screen.getByText(en.lane.red)).toBeInTheDocument();
  });

  it("shows the phase of every proposal as a chip", async () => {
    renderApprovals({
      ...CHANGES,
      items: [CHANGES.items[0], { ...CHANGES.items[1], status: { ...CHANGES.items[1].status, phase: "Deploying" } }],
    });

    await screen.findByText("chg-9f8e7d6c");
    const table = screen.getByRole("table");
    expect(within(table).getByText(en.phase.pendingApproval)).toBeInTheDocument();
    expect(within(table).getByText(en.phase.deploying)).toBeInTheDocument();
  });

  it("narrows the list to the caller's own proposals", async () => {
    const mineToo = proposal({
      metadata: { name: "chg-00000001", namespace: "banskabystrica" },
      author: { name: "Jana Kováčová", email: "Jana.Kovacova@banskabystrica.sk" },
    });
    renderApprovals({ ...CHANGES, items: [...CHANGES.items, mineToo] });

    expect(await screen.findByText("chg-9f8e7d6c")).toBeInTheDocument();
    await userEvent.click(screen.getByLabelText(en.approvals.filterMine));
    expect(screen.getByText("chg-00000001")).toBeInTheDocument();
    expect(screen.queryByText("chg-9f8e7d6c")).toBeNull();
    expect(screen.queryByText("chg-1a2b3c4d")).toBeNull();
  });

  it("narrows the list to one phase, and says so when nothing is left", async () => {
    renderApprovals({
      ...CHANGES,
      items: [CHANGES.items[0], { ...CHANGES.items[1], status: { ...CHANGES.items[1].status, phase: "Deploying" } }],
    });

    await userEvent.selectOptions(await screen.findByLabelText(en.approvals.filterPhase), "Deploying");
    expect(screen.getByText("chg-9f8e7d6c")).toBeInTheDocument();
    expect(screen.queryByText("chg-1a2b3c4d")).toBeNull();

    await userEvent.click(screen.getByLabelText(en.approvals.filterMine));
    expect(screen.getByText(en.approvals.noneMatch)).toBeInTheDocument();
    // The filters stay, so the person can widen the list again.
    expect(screen.getByLabelText(en.approvals.filterMine)).toBeChecked();
  });

  it("attributes each proposal to the person who wrote it", async () => {
    renderApprovals();
    const row = (await screen.findByText("chg-1a2b3c4d")).closest("tr") as HTMLElement;
    expect(within(row).getByText("Marek Mráz")).toBeInTheDocument();
    expect(within(row).getByText(/Mar 3, 2026/)).toBeInTheDocument();
    expect(screen.getByText("Peter Novák")).toBeInTheDocument();
  });

  it("links a proposal to its own detail route", async () => {
    renderApprovals();
    const link = await screen.findByRole("link", {
      name: 'Update Endpoint "air-quality" (2 fields changed)',
    });
    expect(link).toHaveAttribute("href", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  it("says the queue is empty in the table, with the way to a first change (T-1381)", async () => {
    renderApprovals({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [] });
    const empty = await screen.findByText(en.approvals.empty);
    expect(screen.getByRole("table", { name: en.approvals.title })).toContainElement(empty);
    expect(screen.getByRole("link", { name: en.approvals.emptyAction })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/assistant",
    );
  });
});

describe("a bundle in the queue (T-0861)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/approvals");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says how many files a change carries when it carries more than one", async () => {
    renderApprovals({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ChangeList",
      items: [
        proposal({ fileCount: 3 }),
        proposal({
          metadata: { name: "chg-9f8e7d6c", namespace: "banskabystrica" },
          fileCount: 1,
        }),
      ],
    });

    const bundle = (await screen.findByText("chg-1a2b3c4d")).closest("tr") as HTMLElement;
    expect(within(bundle).getByText("3 files")).toBeInTheDocument();
    const single = screen.getByText("chg-9f8e7d6c").closest("tr") as HTMLElement;
    expect(within(single).queryByText(/file/)).not.toBeInTheDocument();
  });
});

/**
 * The page on its own, without the shell around it (T-2137): the two states a list page owes a
 * person before it has any rows — nothing proposed yet, and a read the API refused.
 */
describe("the approvals page, mounted on its own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  /** The page reads the signed-in person to tell their own proposals from everyone else's. */
  const page = <AuthProvider><ApprovalsPage project="helsinki" /></AuthProvider>;

  it("offers the assistant when nothing has been proposed yet", async () => {
    renderPage(page, {
      answer: (url) =>
        url.pathname.endsWith("/changes")
          ? json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [] })
          : undefined,
      path: "/projects/helsinki/approvals",
    });
    expect(await screen.findByText(en.approvals.empty)).toBeInTheDocument();
    expect(screen.getByText(en.approvals.emptyHint)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.approvals.emptyAction })).toBeInTheDocument();
    // No filters over an empty list: there is nothing to narrow.
    expect(screen.queryByLabelText(en.approvals.filterPhase)).not.toBeInTheDocument();
  });

  it("says in the API's own words why the list could not be read, and offers to ask again", async () => {
    renderPage(page, {
      answer: (url) =>
        url.pathname.endsWith("/changes")
          ? problem(403, "You may not read the changes of helsinki.")
          : undefined,
      path: "/projects/helsinki/approvals",
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("You may not read the changes of helsinki.");
    // A refusal answers the same the second time: it says who gives access instead (T-3244).
    expect(alert).toHaveTextContent(en.app.error.forbiddenHint);
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it("offers to ask again when the forge failed", async () => {
    renderPage(page, {
      answer: (url) => (url.pathname.endsWith("/changes") ? problem(503, "The forge does not answer.") : undefined),
      path: "/projects/helsinki/approvals",
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The forge does not answer.");
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });

  it("has no axe violation with rows in it", async () => {
    const { container } = renderPage(page, {
      answer: (url) => (url.pathname.endsWith("/changes") ? json(CHANGES) : undefined),
      path: "/projects/helsinki/approvals",
    });
    await screen.findByText(/air-quality/);
    await expectNoAxeViolations(container);
  });

  // MF-48: a change waiting on another says so in the queue; one already merged is not news.
  it("says which change a proposal waits on, and not one that merged", async () => {
    renderApprovals({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ChangeList",
      items: [
        proposal({
          waitsOn: [
            { name: "chg-00000007", phase: "PendingApproval" },
            { name: "chg-00000005", phase: "Merged" },
          ],
        }),
      ],
    });

    const row = (await screen.findByText("chg-1a2b3c4d")).closest("tr") as HTMLElement;
    expect(
      within(row).getByText(en.approvals.waitsOn.pending.replace("{change}", "chg-00000007")),
    ).toBeInTheDocument();
    expect(within(row).queryByText(/chg-00000005/)).not.toBeInTheDocument();
  });
});

describe("merge requests in the forge that are not Changes (T-3433)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/approvals");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("names each one, with its link to the forge and why", async () => {
    renderApprovals({
      ...CHANGES,
      outside: [
        {
          number: 12,
          url: "https://gitea.example/city/bb-config/pulls/12",
          author: "wera.writer",
          reason: "it comes from a fork; push the branch to the project repository itself",
        },
      ],
    });
    const alert = await screen.findByText("1 merge request in the forge is not a Change");
    const box = alert.closest("[role=status]") as HTMLElement;
    const link = within(box).getByRole("link", { name: /#12 by wera\.writer/ });
    expect(link).toHaveAttribute("href", "https://gitea.example/city/bb-config/pulls/12");
    expect(box.textContent).toContain("it comes from a fork");
  });

  it("says nothing when the API names none", async () => {
    renderApprovals();
    await screen.findByRole("link", { name: 'Remove ContextSpace "doprava"' });
    expect(screen.queryByText(/in the forge (is|are) not/)).toBeNull();
  });
});
