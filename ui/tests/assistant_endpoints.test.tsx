/**
 * The assistant queries the endpoints a person picks (AG-75): the data bar above the text box
 * adds them through a searchable list (keyboard too), holds at most five, sends them with the
 * first message, and in a running conversation sends a changed list with the next message only,
 * following the `endpoints` events the run records; a `query_endpoint` step is drawn as the rows
 * it read, as text only.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectDenied } from "./checks";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { MAX_ENDPOINTS, runEndpointNames, sameEndpoints, storedEndpoints } from "../src/assistant/EndpointPicker";
import { rememberRun } from "../src/assistant/state";
import { ASSISTANT_EVENT } from "../src/navigation/shortcuts";
import { QueryResultCard, queryResultOf, viewOf } from "../src/pages/apps/QueryResultCard";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
const NAMES = ["helsinki-all", "helsinki-kpi", "helsinki-bikes", "helsinki-air", "helsinki-traffic", "helsinki-events"];

function endpoint(name: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, project: PROJECT, title: { en: `${name} title` } },
    spec: {
      slug: `${name.replace(/-/g, "")}slug`,
      contextSpaceRef: "helsinki",
      audience: name === "helsinki-kpi" ? "organization" : "public",
    },
  };
}

class StubEventSource {
  static opened: StubEventSource[] = [];
  private readonly listeners = new Map<string, Set<EventListener>>();

  constructor() {
    StubEventSource.opened.push(this);
  }

  addEventListener(kind: string, listener: EventListener): void {
    const set = this.listeners.get(kind) ?? new Set<EventListener>();
    set.add(listener);
    this.listeners.set(kind, set);
  }

  removeEventListener(kind: string, listener: EventListener): void {
    this.listeners.get(kind)?.delete(listener);
  }

  close(): void {}

  emit(kind: string, payload: Record<string, unknown>): void {
    const frame = new MessageEvent(kind, { data: JSON.stringify(payload), lastEventId: String(payload.seq ?? "") });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

async function emit(kind: string, payload: Record<string, unknown>) {
  await act(async () => {
    for (const stream of StubEventSource.opened) {
      stream.emit(kind, payload);
    }
  });
}

function renderDock() {
  const posted: unknown[] = [];
  const messages: unknown[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
    }
    if (request.method === "POST" && url.pathname.endsWith("/assistant/conversations")) {
      posted.push(JSON.parse(await request.text()));
      return json({ id: RUN_ID, status: "queued" }, 202);
    }
    if (request.method === "POST" && url.pathname.endsWith(`/agent-runs/${RUN_ID}/messages`)) {
      messages.push(JSON.parse(await request.text()));
      return json({}, 202);
    }
    if (url.pathname.endsWith(`/agent-runs/${RUN_ID}`)) {
      return json({
        id: RUN_ID,
        project: PROJECT,
        kind: "conversation",
        appName: "",
        endpointName: "helsinki-all",
        prompt: "Which stations are empty?",
        status: "interviewing",
        steps: 1,
        tokensUsed: 10,
        createdBy: "demo.steward",
        createdAt: "2026-09-13T08:00:00Z",
        endpoints: [{ name: "helsinki-all", slug: "helsinkiallslug", space: "helsinki" }],
      });
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: NAMES.map(endpoint) });
    }
    if (url.pathname.endsWith("/agent-runs")) {
      return json({ items: [] });
    }
    return json({});
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rootRoute = createRootRoute({ component: () => <AssistantDock project={PROJECT} /> });
  const router = createRouter({ routeTree: rootRoute });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { posted, messages };
}

const chips = () => screen.queryByRole("list", { name: en.assistant.data.chosen });
const chipNames = () =>
  within(chips() ?? document.createElement("ul"))
    .queryAllByRole("listitem")
    .map((chip) => chip.getAttribute("title"));
const options = () => within(screen.getByRole("listbox", { name: en.assistant.data.label })).getAllByRole("option");
const option = (name: string) =>
  within(screen.getByRole("listbox", { name: en.assistant.data.label })).getByRole("option", {
    name: new RegExp(`^${name} title`),
  });

beforeEach(async () => {
  window.sessionStorage.clear();
  StubEventSource.opened = [];
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the data bar (AG-75)", () => {
  it("adds endpoints from the list, at most five, removes one by its chip, and sends them with the first message", async () => {
    const user = userEvent.setup();
    const { posted } = renderDock();

    await user.click(await screen.findByRole("button", { name: en.assistant.open }));
    expect(screen.getByText(en.assistant.data.empty)).toBeInTheDocument();
    const add = screen.getByRole("button", { name: en.assistant.data.add });
    await user.click(add);
    expect(screen.getByRole("combobox", { name: en.assistant.data.search })).toHaveFocus();
    await waitFor(() => expect(options()).toHaveLength(NAMES.length));
    expect(option("helsinki-kpi")).toHaveTextContent("helsinki-kpi");
    expect(option("helsinki-kpi")).toHaveTextContent(en.assistant.data.audience.organization);

    for (const name of NAMES) {
      await user.click(option(name));
    }
    expect(options().filter((row) => row.getAttribute("aria-selected") === "true")).toHaveLength(MAX_ENDPOINTS);
    expect(option("helsinki-events")).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("At most 5 endpoints; remove one to add another.")).toBeInTheDocument();
    expect(screen.queryByText(en.assistant.data.empty)).toBeNull();

    expect(chipNames()).toEqual(NAMES.slice(0, MAX_ENDPOINTS));
    expect(within(chips()!).getByText("helsinki-all title")).toBeInTheDocument();
    await user.click(within(chips()!).getByRole("button", { name: "Remove helsinki-kpi" }));
    expect(chipNames()).toEqual(["helsinki-all", "helsinki-bikes", "helsinki-air", "helsinki-traffic"]);
    expect(option("helsinki-events")).toHaveAttribute("aria-disabled", "false");
    expect(storedEndpoints(PROJECT)).toEqual(["helsinki-all", "helsinki-bikes", "helsinki-air", "helsinki-traffic"]);

    await user.type(screen.getByLabelText(en.assistant.empty.composer), "Which stations are empty?");
    await user.click(screen.getByRole("button", { name: en.assistant.empty.send }));
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]).toEqual({
      message: "Which stations are empty?",
      endpointNames: ["helsinki-all", "helsinki-bikes", "helsinki-air", "helsinki-traffic"],
    });
  });

  it("an Endpoint page's Ask the assistant opens it on that one Endpoint and sends it with the first message (T-4760)", async () => {
    const user = userEvent.setup();
    const { posted } = renderDock();
    await screen.findByRole("button", { name: en.assistant.open });
    act(() => {
      window.dispatchEvent(new CustomEvent(ASSISTANT_EVENT, { detail: { endpoints: ["helsinki-air"] } }));
    });
    await waitFor(() => expect(chipNames()).toEqual(["helsinki-air"]));
    expect(storedEndpoints(PROJECT)).toEqual(["helsinki-air"]);
    await user.type(screen.getByLabelText(en.assistant.empty.composer), "Which attributes and units does this have?");
    await user.click(screen.getByRole("button", { name: en.assistant.empty.send }));
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]).toEqual({
      message: "Which attributes and units does this have?",
      endpointNames: ["helsinki-air"],
    });
  });

  it("in a running conversation, Ask on another Endpoint switches it with the next message (T-4760)", async () => {
    const user = userEvent.setup();
    rememberRun({ project: PROJECT, runId: RUN_ID });
    const { messages } = renderDock();
    await waitFor(() => expect(chipNames()).toEqual(["helsinki-all"]));
    act(() => {
      window.dispatchEvent(new CustomEvent(ASSISTANT_EVENT, { detail: { endpoints: ["helsinki-bikes", 7] } }));
    });
    await waitFor(() => expect(chipNames()).toEqual(["helsinki-bikes"]));
    await user.type(screen.getByLabelText(en.agentRun.conversation.placeholder), "And here?");
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.send }));
    await waitFor(() => expect(messages).toHaveLength(1));
    expect(messages[0]).toEqual({ text: "And here?", endpointNames: ["helsinki-bikes"] });
  });

  it("filters by name or title and works from the keyboard", async () => {
    const user = userEvent.setup();
    renderDock();

    await user.click(await screen.findByRole("button", { name: en.assistant.open }));
    const add = screen.getByRole("button", { name: en.assistant.data.add });
    await user.click(add);
    await waitFor(() => expect(options()).toHaveLength(NAMES.length));

    await user.keyboard("air");
    expect(options()).toHaveLength(1);
    await user.clear(screen.getByRole("combobox", { name: en.assistant.data.search }));
    await user.keyboard("bikes title");
    expect(options().map((row) => row.id.split("-").slice(-2).join("-"))).toEqual(["helsinki-bikes"]);

    await user.clear(screen.getByRole("combobox", { name: en.assistant.data.search }));
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowUp}{Enter}");
    expect(chipNames()).toEqual(["helsinki-kpi"]);
    await user.keyboard("{ArrowDown}{Enter}");
    expect(chipNames()).toEqual(["helsinki-kpi", "helsinki-bikes"]);
    await user.keyboard("{Enter}");
    expect(chipNames()).toEqual(["helsinki-kpi"]);

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: en.assistant.data.label })).toBeNull();
    expect(add).toHaveFocus();
    expect(add).toHaveAttribute("aria-expanded", "false");
  });

  // T-2763: "and this one?" is about the page the person is on as they send it.
  it("in a running conversation sends the page each message was written on", async () => {
    const user = userEvent.setup();
    window.history.pushState({}, "", `/projects/${PROJECT}/endpoints/helsinki-all`);
    rememberRun({ project: PROJECT, runId: RUN_ID });
    const { messages } = renderDock();
    await waitFor(() => expect(chipNames()).toEqual(["helsinki-all"]));
    await user.type(screen.getByLabelText(en.agentRun.conversation.placeholder), "what is on this page?");
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.send }));
    await waitFor(() => expect(messages).toHaveLength(1));
    expect(messages[0]).toEqual({
      text: "what is on this page?",
      pageContext: { route: `/projects/${PROJECT}/endpoints/helsinki-all` },
    });
    window.history.pushState({}, "", "/");
  });

  it("in a running conversation sends a changed list with the next message only, follows the run's endpoints events, and takes a found endpoint by Use", async () => {
    const user = userEvent.setup();
    rememberRun({ project: PROJECT, runId: RUN_ID });
    const { messages } = renderDock();

    await waitFor(() => expect(chipNames()).toEqual(["helsinki-all"]));
    const box = screen.getByLabelText(en.agentRun.conversation.placeholder);
    const sendButton = () => screen.getByRole("button", { name: en.agentRun.conversation.send });

    await user.type(box, "How many are empty?");
    await user.click(sendButton());
    await waitFor(() => expect(messages).toHaveLength(1));
    expect(messages[0]).toEqual({ text: "How many are empty?" });

    await user.click(screen.getByRole("button", { name: en.assistant.data.add }));
    await waitFor(() => expect(options()).toHaveLength(NAMES.length));
    await user.keyboard("kpi{Enter}{Escape}");
    expect(chipNames()).toEqual(["helsinki-all", "helsinki-kpi"]);

    await user.type(box, "And the KPI?");
    await user.click(sendButton());
    await waitFor(() => expect(messages).toHaveLength(2));
    expect(messages[1]).toEqual({ text: "And the KPI?", endpointNames: ["helsinki-all", "helsinki-kpi"] });

    await emit("endpoints", { seq: 4, names: ["helsinki-all", "helsinki-kpi"] });
    expect(await screen.findByTestId("endpoints-line")).toHaveTextContent("Data: helsinki-all, helsinki-kpi");
    await user.type(box, "Thanks");
    await user.click(sendButton());
    await waitFor(() => expect(messages).toHaveLength(3));
    expect(messages[2]).toEqual({ text: "Thanks" });

    await emit("endpoints", { seq: 7, names: ["helsinki-bikes"] });
    await waitFor(() => expect(chipNames()).toEqual(["helsinki-bikes"]));

    const found = (name: string) => ({
      kind: "Endpoint",
      name,
      space: "helsinki",
      owner: "helsinki",
      title: `${name} title`,
      matchReason: ["title"],
      access: { verdict: "allowed", reason: "audience public" },
      freshness: null,
    });
    await emit("tool", {
      seq: 8,
      tool: "search_catalog",
      status: "ok",
      input: { words: "air" },
      output: { items: [found("helsinki-air"), found("helsinki-bikes")] },
    });
    const results = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    // Refused and reachable, so the reason is readable where the control is (T-1743, UI-44).
    expectDenied(within(results).getByRole("button", { name: "helsinki-bikes is in use" }));
    await user.click(within(results).getByRole("button", { name: "Use helsinki-air in this conversation" }));
    expect(chipNames()).toEqual(["helsinki-bikes", "helsinki-air"]);
    expectDenied(within(results).getByRole("button", { name: "helsinki-air is in use" }));
  });

  it("compares lists in order and reads a run's endpoint names, ignoring what is not one", () => {
    expect(sameEndpoints(["a", "b"], ["a", "b"])).toBe(true);
    expect(sameEndpoints(["a", "b"], ["b", "a"])).toBe(false);
    expect(sameEndpoints([], ["a"])).toBe(false);
    expect(runEndpointNames([{ name: "a", slug: "x" }, { slug: "y" }, { name: "b" }])).toEqual(["a", "b"]);
    expect(runEndpointNames(undefined)).toEqual([]);
  });
});

describe("the query result card (AG-75)", () => {
  const normalized = [
    {
      id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001",
      type: "BikeHireDockingStation",
      name: { type: "Property", value: "Kaivopuisto" },
      availableBikeNumber: { type: "Property", value: 0 },
      refDistrict: { type: "Relationship", object: "urn:ngsi-ld:District:hel.fi:helsinki:ullanlinna" },
    },
  ];
  const keyValues = { entities: [{ id: "urn:ngsi-ld:Alert:hel.fi:helsinki:a1", type: "Alert", category: "traffic", severity: "high" }] };

  it("draws normalized entities as a table of their local ids and unwrapped values", () => {
    const view = viewOf({ content: [{ type: "text", text: JSON.stringify(normalized) }] });
    expect(view).toEqual({
      kind: "table",
      columns: ["name", "availableBikeNumber", "refDistrict"],
      rows: [{ id: "001", cells: ["Kaivopuisto", "0", "urn:ngsi-ld:District:hel.fi:helsinki:ullanlinna"] }],
      total: 1,
    });
  });

  it("draws keyValues entities from structured content and falls back to the text", () => {
    const table = viewOf({ structuredContent: keyValues, content: [] });
    expect(table.kind).toBe("table");
    expect(viewOf({ content: [{ type: "text", text: "no entity matched" }] })).toEqual({
      kind: "text",
      text: "no entity matched",
    });
    expect(viewOf({ structuredContent: { type: "Alert", attributes: 4 } })).toEqual({
      kind: "fields",
      fields: [
        ["type", "Alert"],
        ["attributes", "4"],
      ],
    });
  });

  it("renders the card as text only and names the endpoint and the argument, never the tool", async () => {
    await i18n.changeLanguage("en");
    const result = queryResultOf({
      tool: "query_endpoint",
      status: "ok",
      input: { endpoint: "helsinki-all", name: "query_entities", arguments: { type: "Alert" } },
      output: {
        structuredContent: {
          entities: [{ id: "urn:ngsi-ld:Alert:hel.fi:helsinki:a1", type: "Alert", headline: "<img src=x onerror=alert(1)>" }],
        },
      },
    });
    expect(result).not.toBeNull();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryResultCard result={result!} />
      </I18nextProvider>,
    );
    const card = screen.getByTestId("query-result");
    expect(card).toHaveTextContent("helsinki-all");
    expect(card).not.toHaveTextContent("query_entities");
    expect(card).toHaveTextContent("type: Alert");
    expect(card).toHaveTextContent("<img src=x onerror=alert(1)>");
    expect(card.querySelector("img")).toBeNull();
    expect(card).toHaveTextContent("1 row");
    expect(queryResultOf({ tool: "query_endpoint", status: "failed", input: {}, error: "403" })).toBeNull();
    expect(queryResultOf({ tool: "compute_kpi", input: {}, output: {} })).toBeNull();
  });
});
