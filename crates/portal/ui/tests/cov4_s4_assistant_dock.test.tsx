// T-7706: the assistant dock's remaining paths: a form's question, pages the assistant opened (notice, trail, links in an answer), Build an app, retry and stop, a table or a model file attached, storage refused.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { askAbout, rememberRun, requestOpen, takePrefill } from "../src/assistant/state";
import { ASSISTANT_EVENT } from "../src/navigation/shortcuts";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "alerts", namespace: PROJECT },
  spec: { contextSpaceRef: "traffic", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project-list", enabledRepresentations: ["ngsi-ld"] },
};
const MODEL_ANSWER = {
  linkml: "name: sensors\n",
  operations: [
    { op: "addClass", name: "Sensors", is_a: "Entity" },
    { op: "addSlot", name: "temp", class: "Sensors", range: "float" },
  ],
  detectedTypes: { temp: "float" },
  matches: {},
  untyped: [],
  rows: 1,
};

class StubEventSource {
  static opened: StubEventSource[] = [];
  private readonly listeners = new Map<string, Set<EventListener>>();
  constructor(readonly url: string) {
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
    for (const listener of [...(this.listeners.get(kind) ?? [])]) listener(frame);
  }
}

interface Seen {
  posted: { path: string; body: unknown }[];
}

function renderDock(options: { propose?: boolean } = {}): Seen {
  const seen: Seen = { posted: [] };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (request.method === "POST") {
        const text = await request.text();
        seen.posted.push({ path: url.pathname, body: text.startsWith("{") ? (JSON.parse(text) as unknown) : text.slice(0, 20) });
      }
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({
          project: PROJECT,
          bootstrap: false,
          grants: [{ role: "r", binding: "b", scope: "project", rule: { kinds: ["*"], verbs: options.propose === false ? ["read"] : ["read", "propose"] } }],
        });
      }
      if (url.pathname.endsWith("/tools/infer-schema")) return json(MODEL_ANSWER);
      if (url.pathname.endsWith(`/agent-runs/${RUN_ID}/cancel`)) return json({ id: RUN_ID, status: "cancelled" });
      if (url.pathname.endsWith("/agent-runs") && request.method === "POST") return json({ id: "01J8ZQ4T7K9M2N3P4Q5R6S7T9Z", appName: "alerts-desk" }, 202);
      if (url.pathname.endsWith(`/agent-runs/${RUN_ID}`)) {
        return json({
          id: RUN_ID,
          project: PROJECT,
          kind: "conversation",
          appName: "",
          endpointName: "",
          prompt: "Which alerts are open?",
          status: "interviewing",
          steps: 1,
          tokensUsed: 10,
          createdBy: "demo.steward",
          createdAt: "2026-09-21T05:00:00Z",
          endpoints: [],
        });
      }
      if (url.pathname.endsWith("/endpoints")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [ENDPOINT] });
      if (url.pathname.endsWith("/access")) return json({ permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity"], attributes: "*" }], prohibitions: [] });
      if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 1 }] });
      if (url.pathname.endsWith("/schema/v1/json-schema")) return json({ $defs: { Alert: { properties: { title: {} } } } });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  const router = createRouter({ routeTree: createRootRoute({ component: () => <AssistantDock project={PROJECT} /> }) });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return seen;
}

const dockStream = () => StubEventSource.opened.at(-1) as StubEventSource;
async function emit(kind: string, payload: Record<string, unknown>) {
  await act(async () => {
    dockStream().emit(kind, payload);
  });
}
async function follow(): Promise<void> {
  await act(async () => {
    rememberRun({ project: PROJECT, runId: RUN_ID });
  });
  await waitFor(() => expect(StubEventSource.opened.length).toBeGreaterThan(0));
}

beforeEach(async () => {
  StubEventSource.opened = [];
  window.sessionStorage.clear();
  window.history.pushState({}, "", "/");
  rememberRun(null);
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  rememberRun(null);
});

describe("what a form or the palette asks of the dock", () => {
  it("puts a form's question in the composer without sending it", async () => {
    const seen = renderDock();
    await screen.findByRole("button", { name: en.assistant.open });
    await act(async () => {
      askAbout("Why is the name refused?");
    });
    expect(await screen.findByLabelText(en.assistant.empty.composer)).toHaveValue("Why is the name refused?");
    expect(seen.posted).toEqual([]);
  });

  it("opens on a request that names no endpoints and keeps the endpoints it had", async () => {
    renderDock();
    await screen.findByRole("button", { name: en.assistant.open });
    await act(async () => {
      window.dispatchEvent(new CustomEvent(ASSISTANT_EVENT, { detail: { text: "bikes" } }));
      window.dispatchEvent(new CustomEvent(ASSISTANT_EVENT, { detail: { endpoints: [] } }));
    });
    expect(await screen.findByLabelText(en.assistant.empty.composer)).toHaveValue("bikes");
  });

  it("opens the app builder on request and goes back to the chat from it", async () => {
    renderDock();
    await screen.findByRole("button", { name: en.assistant.open });
    await act(async () => {
      requestOpen("build");
    });
    const builder = await screen.findByTestId("assistant-build");
    await userEvent.click(within(builder).getByRole("button", { name: en.assistant.backToChat }));
    expect(await screen.findByTestId("assistant-empty")).toBeInTheDocument();
  });
});

describe("a model or a table attached in the dock", () => {
  it("opens the Models page with the drafted source when the person populates the editor", async () => {
    renderDock();
    await userEvent.click(await screen.findByRole("button", { name: en.assistant.open }));
    const file = new File(["id,temp\n1,21.5\n"], "sensors.json", { type: "application/json" });
    await userEvent.upload(await screen.findByLabelText(en.assistant.attach), file);
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: en.models.infer.populate }));
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${PROJECT}/models`));
    const prefill = takePrefill(`/projects/${PROJECT}/models`);
    expect(String(prefill?.source)).toContain("Sensors");
  });

  it("takes a table into the dock and goes back to the chat from it", async () => {
    renderDock();
    await userEvent.click(await screen.findByRole("button", { name: en.assistant.open }));
    const table = new File(["id,temp\n1,21.5\n"], "sensors.csv", { type: "text/csv" });
    await userEvent.upload(await screen.findByLabelText(en.assistant.attach), table);
    const card = await screen.findByTestId("assistant-table");
    await userEvent.click(within(card).getByRole("button", { name: en.assistant.backToChat }));
    expect(await screen.findByTestId("assistant-empty")).toBeInTheDocument();
  });
});

describe("a conversation the assistant drives", () => {
  it("tells the person which page it opened, lists the pages, and lets them dismiss or go back", async () => {
    renderDock();
    await follow();
    await emit("navigate", { seq: 1, route: `/projects/${PROJECT}/pipelines` });
    await emit("navigate", { seq: 2, route: `/projects/${PROJECT}/endpoints` });
    const trail = await screen.findByTestId("assistant-trail");
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${PROJECT}/endpoints`));
    const buttons = within(trail).getAllByRole("button");
    expect(buttons.length).toBe(2);
    await userEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${PROJECT}/pipelines`));
    const dismiss = screen.queryByRole("button", { name: en.assistant.dismiss });
    if (dismiss) {
      await userEvent.click(dismiss);
      await waitFor(() => expect(screen.queryByRole("button", { name: en.assistant.dismiss })).toBeNull());
    }
  });

  it("dismisses the notice of a page it opened", async () => {
    renderDock();
    await follow();
    await emit("navigate", { seq: 1, route: `/projects/${PROJECT}/pipelines` });
    await userEvent.click(await screen.findByRole("button", { name: en.assistant.dismiss }));
    await waitFor(() => expect(screen.queryByRole("button", { name: en.assistant.dismiss })).toBeNull());
  });

  it("opens a page named in an answer in place, and starts the app a Build card describes", async () => {
    const seen = renderDock();
    await follow();
    await emit("message", { seq: 1, text: "Which alerts are open?", sentBy: "demo.steward" });
    await emit("thought", { seq: 2, text: `See [the pipelines](/projects/${PROJECT}/pipelines) for them.` });
    await userEvent.click(await screen.findByRole("link", { name: "the pipelines" }));
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${PROJECT}/pipelines`));
    await emit("app-build", { seq: 3, endpoints: ["alerts"], access: "read", visibility: "project", prompt: "A desk for the alerts" });
    const build = await screen.findByRole("button", { name: en.apps.buildCard.build });
    await waitFor(() => expect(build).toBeEnabled());
    await userEvent.click(build);
    await waitFor(() => expect(seen.posted.some((one) => one.path.endsWith("/agent-runs"))).toBe(true));
    // The dock follows the run the card started.
    await waitFor(() => expect(window.sessionStorage.getItem("jc.assistant.run")).toContain("01J8ZQ4T7K9M2N3P4Q5R6S7T9Z"));
  });

  it("opens a page named in an answer beside the conversation when the dock is full screen", async () => {
    window.sessionStorage.setItem("jc.assistant.layout", "full");
    renderDock();
    await follow();
    await emit("thought", { seq: 1, text: `See [the endpoints](/projects/${PROJECT}/endpoints).` });
    await userEvent.click(await screen.findByRole("link", { name: "the endpoints" }));
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/${PROJECT}/endpoints`));
    await waitFor(() => expect(screen.getByRole("complementary").getAttribute("data-layout")).toBe("side"));
  });

  it("offers to ask again and to stop once the answer is long overdue", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const seen = renderDock();
    await follow();
    await emit("status", { seq: 1, status: "interviewing" });
    await emit("message", { seq: 3, text: "Which alerts are open?", sentBy: "demo.steward" });
    await emit("tool", { seq: 4, tool: "search_catalog", status: "ok", durationMs: 0 });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50_000);
    });
    const person = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await person.click(await screen.findByRole("button", { name: en.agentRun.conversation.stop }));
    await waitFor(() => expect(seen.posted.some((one) => one.path.endsWith("/cancel"))).toBe(true));
    await person.click(screen.getByRole("button", { name: en.agentRun.conversation.retry }));
    // Asking again leaves the stalled conversation and starts one with the same question.
    await waitFor(() =>
      expect(seen.posted.some((one) => one.path.endsWith("/assistant/conversations") && JSON.stringify(one.body).includes("Which alerts are open?"))).toBe(true),
    );
  });
});

describe("a browser that refuses its storage", () => {
  it("opens the dock at the side when the layout cannot be read", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("storage is blocked");
    });
    renderDock();
    await userEvent.click(await screen.findByRole("button", { name: en.assistant.open }));
    expect((await screen.findByRole("complementary")).getAttribute("data-layout")).toBe("side");
  });
});
