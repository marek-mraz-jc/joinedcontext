// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/assistant/AssistantPage.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { onOpenRequest } from "../src/assistant/state";
import { AssistantPage } from "../src/pages/assistant/AssistantPage";
import { expectHeadingOutline, expectNoAxeViolations, json, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
  // A member of the project: its endpoints' `project-list` audience admits her (EP-14, T-2631).
  groups: [PROJECT],
};

const CONV_RUN = {
  id: "run-conv-1",
  project: PROJECT,
  appName: "",
  endpointName: "",
  endpointSlug: "",
  profile: "app-builder",
  appClass: "static",
  visibility: "private",
  kind: "conversation",
  unattended: false,
  continues: null,
  prompt: "Find all air quality sensors in town",
  status: "cancelled",
  steps: 2,
  tokensUsed: 500,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T09:00:00Z",
};

const WORK_RUN = {
  id: "run-work-1",
  project: PROJECT,
  appName: "city-bikes-app",
  endpointName: "bikes-endpoint",
  endpointSlug: "slug123",
  profile: "app-builder",
  appClass: "static",
  visibility: "project",
  kind: "application",
  unattended: true,
  continues: null,
  prompt: "Create city bikes overview",
  status: "awaiting_approval",
  firstFrameMs: 1200,
  firstVersionMs: 2500,
  steps: 5,
  tokensUsed: 2500,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T10:00:00Z",
};

const CONTINUED_RUN = {
  id: "run-conv-2",
  project: PROJECT,
  appName: "",
  endpointName: "",
  endpointSlug: "",
  profile: "app-builder",
  appClass: "static",
  visibility: "private",
  kind: "conversation",
  unattended: false,
  continues: "run-conv-1",
  prompt: "Continue sensor analysis",
  status: "interviewing",
  steps: 1,
  tokensUsed: 200,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T11:00:00Z",
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "bikes-endpoint" },
      spec: {
        slug: "slug123",
        contextSpaceRef: { kind: "ContextSpace", name: "bikes" },
        enabledRepresentations: ["ngsi-ld"],
      },
    },
  ],
};

class StubEventSource {
  addEventListener(): void {}
  removeEventListener(): void {}
  close(): void {}
}

let requests: Request[] = [];
const fetchCalls = () => requests;

/** What the endpoint publishes, as `/schema/v1/json-schema` answers it. */
const SCHEMA = {
  $defs: {
    Entity: { properties: { id: {} } },
    BikeHireDockingStation: { properties: { availableBikeNumber: {}, name: {} } },
  },
};

function renderAssistantPage(
  runs: unknown[] = [CONV_RUN, WORK_RUN, CONTINUED_RUN],
  postRunHandler?: (req: Request) => Promise<Response>,
  schema: unknown = SCHEMA,
) {
  requests = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init);
    requests.push(request);
    const url = new URL(request.url, "http://localhost");
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/projects")) {
      return json([PROJECT]);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/schema/index.json")) {
      return json({ models: [{ version: 1 }] });
    }
    if (path.endsWith("/json-schema")) {
      return json(schema);
    }
    if (path.endsWith("/assistant/conversations") && request.method === "POST") {
      return json(
        {
          id: "new-conv-run-id",
          project: PROJECT,
          appName: "",
          endpointName: "",
          endpointSlug: "",
          appClass: "static",
          visibility: "private",
          kind: "conversation",
          unattended: false,
          prompt: en.assistantPage.continueMessage,
          status: "interviewing",
          steps: 0,
          tokensUsed: 0,
          createdBy: "jana.kovacova",
          createdAt: "2026-09-14T12:00:00Z",
        },
        202,
      );
    }
    if (path.endsWith("/agent-runs")) {
      if (request.method === "POST") {
        if (postRunHandler) {
          return postRunHandler(request);
        }
        return json(
          {
            id: "new-work-run-id",
            project: PROJECT,
            appName: "new-work-app",
            endpointName: "bikes-endpoint",
            endpointSlug: "slug123",
            appClass: "static",
            visibility: "project",
            kind: "dashboard",
            unattended: true,
            prompt: "Create bike dashboard",
            status: "queued",
            steps: 0,
            tokensUsed: 0,
            createdBy: "jana.kovacova",
            createdAt: "2026-09-14T12:00:00Z",
          },
          202,
        );
      }
      // As the Portal answers it (AG-93): the journeys' test runs only when asked for.
      const shown = url.searchParams.get("origin") === "all"
        ? runs
        : runs.filter((run) => (run as { origin?: string }).origin !== "journey");
      return json({ items: shown });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });

  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("Assistant page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists a conversation and a work run with their kind labels", async () => {
    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    expect(await screen.findByText("Find all air quality sensors in town")).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getAllByText(i18n.t("assistantPage.kinds.conversation"))).toHaveLength(2);

    expect(within(table).getByText("City bikes app")).toBeInTheDocument();
    expect(within(table).queryByText("city-bikes-app")).not.toBeInTheDocument();
    // One header row whose five columns line up with every body row's cells.
    const rows = within(table).getAllByRole("row");
    expect(within(rows[0]).getAllByRole("columnheader")).toHaveLength(5);
    for (const row of rows.slice(1)) {
      expect(within(row).getAllByRole("cell")).toHaveLength(5);
    }
    expect(within(table).getByText(i18n.t("assistantPage.kinds.application"))).toBeInTheDocument();

    expect(within(table).getByText("Continue sensor analysis")).toBeInTheDocument();
    expect(within(table).getByText(i18n.t("assistantPage.continues"))).toBeInTheDocument();
  });

  // T-2760: every run ever made, listed at once, made the page ten thousand pixels tall.
  it("lists twenty runs at a time and offers the rest", async () => {
    const many = Array.from({ length: 25 }, (_, index) => ({
      ...CONV_RUN,
      id: `run-${index}`,
      prompt: `Question number ${index}`,
    }));
    renderAssistantPage(many);
    const table = await screen.findByRole("table");
    await within(table).findByText("Question number 0");
    expect(within(table).getAllByRole("row")).toHaveLength(21);
    const more = screen.getByRole("button", { name: "Show 5 more (5 left)" });
    await userEvent.click(more);
    expect(within(table).getAllByRole("row")).toHaveLength(26);
    expect(screen.queryByRole("button", { name: /Show \d+ more/ })).toBeNull();
  });

  // T-4994: forty rebuilds of one dashboard read as one row with a count, its timing on one line.
  it("groups one App's runs under its newest with a count and opens the rest on request", async () => {
    const builds = [0, 1, 2].map((index) => ({
      ...WORK_RUN,
      id: `build-${index}`,
      status: "stopped",
      firstFrameMs: 38800 + index,
      firstVersionMs: index === 0 ? 44000 : 38800 + index,
      createdAt: `2026-09-14T1${3 - index}:00:00Z`,
    }));
    renderAssistantPage([...builds, CONV_RUN]);
    const table = await screen.findByRole("table");
    await within(table).findByText("City bikes app");
    expect(within(table).getAllByRole("row")).toHaveLength(3);
    expect(within(table).getByText("First frame 38.8 s · First version 44.0 s")).toBeInTheDocument();
    const all = within(table).getByRole("button", { name: i18n.t("assistantPage.group.all", { count: 3 }) });
    expect(all).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(all);
    expect(within(table).getAllByRole("row")).toHaveLength(5);
    expect(within(table).getAllByText(`${i18n.t("assistantPage.timing.both")} 38.8 s`)).toHaveLength(2);
    await userEvent.click(within(table).getByRole("button", { name: i18n.t("assistantPage.group.latestOnly") }));
    expect(within(table).getAllByRole("row")).toHaveLength(3);
  });

  // T-2816, AG-93: the live journeys' runs crowded the history; they wait behind a tick.
  it("leaves the journeys' test runs out until Show test runs is ticked, and marks them", async () => {
    const journey = { ...CONV_RUN, id: "journey-run", prompt: "Create a SyncSource called t1523", origin: "journey" };
    renderAssistantPage([CONV_RUN, journey]);
    const table = await screen.findByRole("table");
    await within(table).findByText("Find all air quality sensors in town");
    expect(within(table).queryByText("Create a SyncSource called t1523")).toBeNull();

    await userEvent.click(screen.getByRole("checkbox", { name: en.assistantPage.filters.testRuns }));
    // A new query: the table is drawn again once the answer is in.
    const row = (await screen.findByText("Create a SyncSource called t1523")).closest("tr") as HTMLElement;
    expect(within(row).getByText(en.assistantPage.testRun)).toBeInTheDocument();
    expect(
      fetchCalls().some((req) => new URL(req.url, "http://localhost").searchParams.get("origin") === "all"),
    ).toBe(true);
    // A person's own run carries no mark.
    const own = screen.getByText("Find all air quality sensors in town").closest("tr") as HTMLElement;
    expect(within(own).queryByText(en.assistantPage.testRun)).toBeNull();
  });

  it("puts kind= in the request when filtering by kind", async () => {
    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const kindSelect = screen.getByLabelText(i18n.t("assistantPage.filters.kind"));
    const user = userEvent.setup();
    await user.selectOptions(kindSelect, "conversation");

    await waitFor(() => {
      expect(
        fetchCalls().some((req) => {
          const url = new URL(req.url, "http://localhost");
          return url.pathname.endsWith("/agent-runs") && url.searchParams.get("kind") === "conversation";
        }),
      ).toBe(true);
    });
  });

  it("remembers the run in sessionStorage and fires open request when Open is clicked", async () => {
    let opened = false;
    const unsub = onOpenRequest(() => {
      opened = true;
    });

    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const promptEl = await screen.findByText("Find all air quality sensors in town");
    const row = (promptEl.closest("tr") ?? promptEl.closest("li")) as HTMLElement;
    const openButton = within(row).getByRole("button", { name: i18n.t("assistantPage.open") });

    const user = userEvent.setup();
    await user.click(openButton);

    expect(opened).toBe(true);
    const stored = JSON.parse(window.sessionStorage.getItem("jc.assistant.run") ?? "{}") as {
      project: string;
      runId: string;
    };
    expect(stored.runId).toBe(CONV_RUN.id);
    expect(stored.project).toBe(PROJECT);

    unsub();
  });

  it("posts continues when Continue is clicked on an ended conversation", async () => {
    let opened = false;
    const unsub = onOpenRequest(() => {
      opened = true;
    });

    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const promptEl = await screen.findByText("Find all air quality sensors in town");
    const row = (promptEl.closest("tr") ?? promptEl.closest("li")) as HTMLElement;
    const continueButton = within(row).getByRole("button", {
      name: i18n.t("assistantPage.continue"),
    });

    const user = userEvent.setup();
    await user.click(continueButton);

    await waitFor(() => {
      const postReq = fetchCalls().find(
        (req) => req.method === "POST" && req.url.includes("/assistant/conversations"),
      );
      expect(postReq).toBeDefined();
    });

    const postReq = fetchCalls().find(
      (req) => req.method === "POST" && req.url.includes("/assistant/conversations"),
    )!;
    const body = (await postReq.json()) as { continues: string; message: string };
    expect(body.continues).toBe(CONV_RUN.id);
    expect(body.message).toBe(i18n.t("assistantPage.continueMessage"));

    expect(opened).toBe(true);
    const stored = JSON.parse(window.sessionStorage.getItem("jc.assistant.run") ?? "{}") as {
      project: string;
      runId: string;
    };
    expect(stored.runId).toBe("new-conv-run-id");
    expect(stored.project).toBe(PROJECT);

    unsub();
  });

  // T-2745, UI-55: the owner removed the New work form. Work starts from the assistant's paths,
  // so the page offers no kind, name, endpoint or prompt field and never starts a run itself.
  it("has no New work form and starts no run of its own", async () => {
    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });
    expect(screen.queryByText("New work")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Start/ })).toBeNull();
    expect(
      fetchCalls().find((req) => req.method === "POST" && req.url.endsWith("/agent-runs")),
    ).toBeUndefined();
  });
});

/**
 * The page on its own (T-2137): what it says before any run exists, and that the list it shows
 * is the one the API answered — the filters narrow what is already there and ask for nothing a
 * person may not see.
 */
describe("the assistant page, mounted on its own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const mounted = (answer: (url: URL) => Response | undefined) =>
    renderPage(<AssistantPage project="helsinki" />, {
      answer,
      path: "/projects/helsinki/assistant",
    });

  it("says there is nothing yet, and offers the way to start", async () => {
    mounted((url) => (url.pathname.endsWith("/agent-runs") ? json({ items: [] }) : undefined));
    expect(await screen.findByRole("heading", { level: 1, name: en.assistantPage.title })).toBeInTheDocument();
    // The header is there before the list is: the empty state is what the answer produces.
    expect(await screen.findByText(en.assistantPage.empty)).toBeInTheDocument();
    // The way to start is the assistant, and the empty page opens it; it once pointed at a New
    // work form "below" that T-2745 had taken away.
    let opened = false;
    const unsub = onOpenRequest(() => {
      opened = true;
    });
    await userEvent.setup().click(screen.getByRole("button", { name: en.assistant.open }));
    expect(opened).toBe(true);
    unsub();
  });

  it("asks for the project in the address and for no other", async () => {
    const asked: string[] = [];
    mounted((url) => {
      asked.push(url.pathname);
      return url.pathname.endsWith("/agent-runs") ? json({ items: [] }) : undefined;
    });
    await screen.findByText(en.assistantPage.empty);
    expect(asked.some((path) => path.startsWith("/api/v1/projects/helsinki/"))).toBe(true);
    expect(asked.some((path) => /\/projects\/(?!helsinki\b)[^/]+\//.test(path))).toBe(false);
  });

  it("has no axe violation and one H1", async () => {
    const { container } = mounted((url) =>
      url.pathname.endsWith("/agent-runs") ? json({ items: [] }) : undefined,
    );
    await screen.findByText(en.assistantPage.empty);
    expectHeadingOutline(container);
    await expectNoAxeViolations(container);
  });
});
