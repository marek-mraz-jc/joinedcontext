/**
 * One builder run, watched (T-0539, AG-43…AG-46, UI-34…UI-40).
 *
 * The things worth asserting are the ones a person cannot check by looking: that a reconnect's
 * replay does not double every line, that publishing is offered only when there is something to
 * publish, and that the preview frame is not handed the reviewer's own session.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { AgentRunPage } from "../src/pages/apps/AgentRunPage";
import { AssistantDock } from "../src/assistant/AssistantDock";

const PROJECT = "banskabystrica";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";

/** The run's name read as words, never its id; the endpoint's title follows when it is known. */
const APP_TITLE = "Ovzdusie dnes";

const RUN = {
  id: RUN_ID,
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 12,
  tokensUsed: 48_210,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

/** The Change a Publish opened, written by the person watching the run (AP-71, PF-58). */
const PUBLICATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-00000069", namespace: PROJECT },
  summary: { key: "change.summary.create", params: { kind: "App", name: "ovzdusie-dnes" } },
  author: { name: "Jana Kováčová", email: "jana.kovacova@banskabystrica.sk" },
  createdAt: "2026-09-12T08:05:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

/**
 * The browser's `EventSource`, as much of it as the hook uses. A stub rather than a mock server
 * because the interesting cases are the frames themselves: their order, their duplicates, and
 * the ones the Portal did not write.
 */
class StubEventSource {
  static opened: StubEventSource[] = [];
  readonly url: string;
  readonly withCredentials: boolean;
  closed = false;
  private readonly listeners = new Map<string, Set<EventListener>>();

  constructor(url: string, init?: EventSourceInit) {
    this.url = url;
    this.withCredentials = init?.withCredentials ?? false;
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

  close(): void {
    this.closed = true;
  }

  /** One named frame, exactly as the Portal writes it: `event:` the kind, `data:` the JSON. */
  emit(kind: string, payload: unknown, data?: string): void {
    const frame = new MessageEvent(kind, {
      data: data ?? JSON.stringify(payload),
      lastEventId: String((payload as { seq?: number } | undefined)?.seq ?? ""),
    });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

interface Options {
  run?: Record<string, unknown>;
  write?: { body: unknown; status: number };
}

function renderRun(options: Options = {}) {
  const { run = RUN, write = { body: { ok: true }, status: 200 } } = options;
  let current = run;

  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) {
      return json({
        subject: "b7c1e0f4",
        username: "jana.kovacova",
        email: "jana.kovacova@banskabystrica.sk",
        roles: ["domain-editor"],
      });
    }
    if (url.pathname.endsWith("/permissions/me")) {
      return json({
        project: PROJECT,
        bootstrap: false,
        grants: [
          {
            role: "org-admin",
            binding: "admins",
            rule: { kinds: ["App"], verbs: ["propose", "approve", "delete"] },
          },
        ],
      });
    }
    if (url.pathname.includes("/changes/") && request.method === "GET") {
      return json(PUBLICATION);
    }
    if (request.method === "POST") {
      return json(write.body, write.status);
    }
    return json(current);
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The page and the shell's assistant dock together, the way the Portal shows a run: the
  // conversation is the dock's, and the dock needs a router to follow a navigate frame.
  const rootRoute = createRootRoute({
    component: () => (
      <>
        <AgentRunPage project={PROJECT} runId={RUN_ID} onClose={() => {}} />
        <AssistantDock project={PROJECT} />
      </>
    ),
  });
  const router = createRouter({ routeTree: rootRoute });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return {
    fetchMock,
    view,
    /** What the next re-read of the record answers with, the way a status event makes it change. */
    setRun: (next: Record<string, unknown>) => {
      current = next;
    },
  };
}

const stream = () => StubEventSource.opened.at(-1) as StubEventSource;

/** A frame reaches every open stream of the run: the page's and the dock's. */
async function emit(kind: string, payload: unknown, data?: string) {
  await act(async () => {
    for (const source of StubEventSource.opened) {
      source.emit(kind, payload, data);
    }
  });
}

describe("watching a run", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    StubEventSource.opened = [];
    window.sessionStorage.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("subscribes to the run's own stream with the session cookie", async () => {
    renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    expect(stream().url).toBe(`/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/events`);
    // The stream is behind the same edge login as the Portal, so it needs the cookie.
    expect(stream().withCredentials).toBe(true);
  });

  it("heads the page with the run's title, keeping the name only as small print", async () => {
    renderRun({ run: { ...RUN, title: "Air quality today" } });
    expect(await screen.findByRole("heading", { name: "Air quality today" })).toBeInTheDocument();
    const id = screen.getByText(RUN.appName);
    expect(id).toHaveClass("font-mono");
    expect(id.tagName).not.toMatch(/^H\d$/);
  });

  // UI-38: the live timeline of the run's lifecycle phases.
  it("shows the state the run is in, and the steps it has spent", async () => {
    renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    const timeline = screen.getByRole("region", { name: en.agentRun.timeline.title });
    const current = within(timeline).getByText(en.agentRun.states.building);
    expect(current).toHaveAttribute("aria-current", "step");
    expect(
      within(timeline).getByText(
        en.agentRun.timeline.usage.replace("{steps}", "12").replace("{tokens}", "48,210"),
      ),
    ).toBeInTheDocument();
  });

  it("says an expired run has ended and keeps its preview, with no failure line (T-0669)", async () => {
    renderRun({ run: { ...RUN, status: "expired", previewUrl: "/api/v1/projects/helsinki/agent-runs/run-1/preview?v=1" } });
    await screen.findByRole("heading", { name: APP_TITLE });

    const timeline = screen.getByRole("region", { name: en.agentRun.timeline.title });
    const ended = within(timeline).getByRole("status");
    expect(ended).toHaveTextContent(en.agentRun.states.expired);
    expect(ended.className).not.toContain("text-danger");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(en.agentRun.states.expired).not.toMatch(/out of time|failed/i);
  });

  // UI-38: the ordered, streaming conversation of the run.
  it("renders the agent's lines as they arrive", async () => {
    renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    await emit("thought", { seq: 1, text: "Reading the projected schema" });
    await emit("tool", { seq: 2, tool: "bash", command: "cargo test", exitCode: 0 });

    // The building panel repeats the latest step; the conversation is where the line belongs.
    const conversation = await screen.findByRole("region", { name: en.agentRun.conversation.title });
    expect(await within(conversation).findByText("Reading the projected schema")).toBeInTheDocument();
    // A tool line is an inspectable step (AG-56): its name on the summary, its command inside.
    // The data bar above the text box is a group too; the step is the one that holds the tool.
    const step = within(conversation).getAllByRole("group").find((group) => group.tagName === "DETAILS") as HTMLElement;
    expect(within(step).getByText("bash")).toBeInTheDocument();
    expect(within(step).getByRole("img", { name: en.agentRun.step.ok })).toBeInTheDocument();
    await userEvent.click(within(step).getByText("bash"));
    expect(within(step).getByText("cargo test")).toBeInTheDocument();
  });

  it("does not double a line the reconnect replayed (AG-45)", async () => {
    renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    await emit("thought", { seq: 7, text: "Wiring the map" });
    // What a resume from `Last-Event-ID` brings back: the same frame, the same seq.
    await emit("thought", { seq: 7, text: "Wiring the map" });

    const conversation = await screen.findByRole("region", { name: en.agentRun.conversation.title });
    expect(await within(conversation).findAllByText("Wiring the map")).toHaveLength(1);
    expect(within(screen.getByTestId("run-building")).getAllByText("Wiring the map")).toHaveLength(1);
  });

  it("drops a frame it cannot parse instead of rendering half of it", async () => {
    renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    await emit("thought", undefined, "not json at all");

    expect(screen.getByText(en.agentRun.conversation.empty)).toBeInTheDocument();
  });

  it("re-reads the record when the run changes state rather than polling", async () => {
    const { setRun } = renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    setRun({ ...RUN, status: "testing" });
    await emit("status", { seq: 3, status: "testing" });

    const timeline = screen.getByRole("region", { name: en.agentRun.timeline.title });
    await waitFor(() => {
      expect(within(timeline).getByText(en.agentRun.states.testing)).toHaveAttribute(
        "aria-current",
        "step",
      );
    });
  });

  // UI-38: a question form generated from the JSON Schema the agent sent.
  it("answers a question against the schema the agent asked for", async () => {
    const user = userEvent.setup();
    const { fetchMock } = renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    await emit("question", {
      seq: 4,
      questionId: "q-title",
      schema: {
        type: "object",
        required: ["title"],
        properties: { title: { type: "string", title: "Page title" } },
      },
    });

    await user.type(await screen.findByLabelText(/Page title/), "Air today");
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.answer }));

    await waitFor(async () => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.method === "POST");
      expect(request?.url).toContain(`/agent-runs/${RUN_ID}/answers`);
      const body = (await request!.json()) as Record<string, unknown>;
      expect(body.questionId).toBe("q-title");
      expect(body.answers).toEqual({ title: "Air today" });
    });
  });

  it("stops asking once the question is answered", async () => {
    renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    await emit("question", {
      seq: 5,
      questionId: "q-title",
      schema: { type: "object", properties: { title: { type: "string" } } },
    });
    expect(await screen.findByText(en.agentRun.conversation.question)).toBeInTheDocument();

    await emit("answer", { seq: 6, questionId: "q-title" });

    await waitFor(() => {
      expect(screen.queryByText(en.agentRun.conversation.question)).not.toBeInTheDocument();
    });
  });

  it("offers publishing only once there is a preview to judge (AP-55)", async () => {
    const { setRun } = renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    const publish = screen.getByRole("button", { name: en.agentRun.publish });
    expect(publish).toBeDisabled();

    setRun({ ...RUN, status: "previewing", previewUrl: "/apps/ovzdusie-dnes-preview/" });
    await emit("status", { seq: 8, status: "previewing" });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeEnabled();
    });
  });

  it("holds Publish while the version's tests run or fail, and names the failing ones (SDK-38)", async () => {
    const previewUrl = `/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/preview?v=2`;
    renderRun({ run: { ...RUN, status: "previewing", previewUrl } });
    await screen.findByRole("heading", { name: APP_TITLE });
    expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeEnabled();

    await emit("tests", { seq: 10, version: 2, outcome: "running" });
    const publish = await screen.findByRole("button", { name: en.agentRun.publish });
    await waitFor(() => expect(publish).toBeDisabled());
    expect(publish).toHaveAccessibleDescription(en.agentRun.testsHold.running.replace("{version}", "2"));
    expect(screen.getByText(en.agentRun.line.testsRunning.replace("{version}", "2"))).toBeInTheDocument();

    await emit("tests", {
      seq: 11,
      version: 2,
      outcome: "failed",
      passed: 2,
      failed: 1,
      failures: [{ file: "src/App.test.tsx", name: "lists the stations", message: "expected 3 rows" }],
    });
    await waitFor(() =>
      expect(publish).toHaveAccessibleDescription(/src\/App\.test\.tsx › lists the stations/),
    );
    expect(publish).toBeDisabled();

    // An older version's result is past; the newest of the version on screen decides.
    await emit("tests", { seq: 12, version: 1, outcome: "failed", failed: 4 });
    await emit("tests", { seq: 13, version: 2, outcome: "passed", passed: 3, failed: 0 });
    await waitFor(() => expect(publish).toBeEnabled());
    expect(publish).not.toHaveAttribute("aria-describedby");
  });

  it("publishes a version whose tests were skipped or could not run (SDK-38)", async () => {
    const previewUrl = `/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/preview?v=3`;
    renderRun({ run: { ...RUN, status: "previewing", previewUrl } });
    await screen.findByRole("heading", { name: APP_TITLE });
    await emit("tests", { seq: 10, version: 3, outcome: "skipped", reason: "this installation has no test sandbox" });
    await emit("tests", { seq: 11, version: 3, outcome: "error", reason: "the tests did not finish within 150 s" });
    expect(
      await screen.findByText(/Version 3 was not tested here: this installation has no test sandbox/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeEnabled();
  });

  it("publishes an unattended run that waits for approval with its preview built (AG-69)", async () => {
    renderRun({
      run: { ...RUN, unattended: true, status: "awaiting_approval", previewUrl: "/apps/x/" },
    });
    await screen.findByRole("heading", { name: APP_TITLE });

    expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeEnabled();
  });

  it("links the source in Git and approves its own publication in place as an administrator (AP-71, PF-58)", async () => {
    const { fetchMock } = renderRun({
      run: {
        ...RUN,
        unattended: true,
        status: "awaiting_approval",
        previewUrl: "/apps/x/",
        changeId: "chg-00000069",
        sourceUrl: "https://city.example/git/joinedcontext/configuration/src/branch/agent/ovzdusie-dnes/apps/ovzdusie-dnes",
      },
      write: { body: { ...PUBLICATION, status: { ...PUBLICATION.status, phase: "Deploying" } }, status: 202 },
    });
    await screen.findByRole("heading", { name: APP_TITLE });

    const panel = screen.getByRole("region", { name: en.agentRun.publication.title });
    expect(within(panel).getByRole("link", { name: en.agentRun.publication.source })).toHaveAttribute(
      "href",
      expect.stringContaining("/src/branch/agent/ovzdusie-dnes/"),
    );
    // Already published: the run's Change stands for it, so Publish is not offered twice.
    expect(await screen.findByRole("button", { name: en.agentRun.publication.approve })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: en.agentRun.publication.approve }));
    await waitFor(() => {
      const approvals = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .filter((request) => request.method === "POST" && request.url.includes("/changes/chg-00000069/approve"));
      expect(approvals).toHaveLength(1);
    });
    expect(await within(panel).findByText(en.phase.deploying)).toBeInTheDocument();
  });

  it("frames the preview without handing it the reviewer's session (AP-19)", async () => {
    renderRun({ run: { ...RUN, status: "previewing", previewUrl: "/apps/ovzdusie-dnes-preview/" } });
    await screen.findByRole("heading", { name: APP_TITLE });

    const frame = await screen.findByTitle(
      en.agentRun.preview.frameTitle.replace("{app}", APP_TITLE),
    );
    expect(frame).toHaveAttribute("src", "/apps/ovzdusie-dnes-preview/");
    // `allow-scripts` with `allow-same-origin` on a same-origin frame is not a sandbox: the
    // frame could read the CSRF cookie and write as the person reviewing it. `allow-forms` is
    // what lets an application's Save reach its submit handler (T-1219); nothing else.
    expect(frame.getAttribute("sandbox")?.split(/\s+/).sort()).toEqual(["allow-forms", "allow-scripts"]);
  });

  it("builds in the frame's place with the agent's step and the types it reads, never a template (SDK-14)", async () => {
    renderRun({
      run: {
        ...RUN,
        status: "building",
        dataNeeds: [{ types: ["BikeHireDockingStation", "Entity"], operations: ["queryEntity"] }],
      },
    });
    await screen.findByRole("heading", { name: APP_TITLE });
    await emit("thought", { seq: 3, text: "Reading 5 entities of BikeHireDockingStation through the endpoint." });

    const building = await screen.findByTestId("run-building");
    expect(within(building).getByText(en.agentRun.preview.building)).toBeInTheDocument();
    expect(within(building).getByText("Reading BikeHireDockingStation")).toBeInTheDocument();
    expect(within(building).getByTestId("run-building-thought")).toHaveTextContent(
      "Reading 5 entities of BikeHireDockingStation through the endpoint.",
    );
    expect(screen.queryByTitle(en.agentRun.preview.frameTitle.replace("{app}", APP_TITLE))).toBeNull();
  });

  it("says a live run without a version could not build yet, with its errors and how to retry (SDK-14)", async () => {
    renderRun({ run: { ...RUN, status: "previewing" } });
    await screen.findByRole("heading", { name: APP_TITLE });
    await emit("thought", { seq: 4, text: "The application still does not build:\nsrc/App.tsx:3 Cannot find name 'Row'" });

    const building = await screen.findByTestId("run-building");
    expect(within(building).getByText(en.agentRun.preview.notBuilt)).toBeInTheDocument();
    expect(within(building).getByText(en.agentRun.preview.retry)).toBeInTheDocument();
    expect(within(building).getByTestId("run-building-thought")).toHaveTextContent("Cannot find name 'Row'");
    expect(within(building).queryByText(en.agentRun.preview.building)).toBeNull();
  });

  it("watches the first minute on the left, then shows each pass in a fresh frame (UI-41, UI-42)", async () => {
    const { setRun } = renderRun({
      run: { ...RUN, status: "building", createdAt: new Date(Date.now() - 12_000).toISOString() },
    });
    await screen.findByRole("heading", { name: APP_TITLE });

    // Before the first pass the preview column says which phase the run is in and how long it
    // has been running, rather than standing empty.
    const preview = screen.getByRole("region", { name: en.agentRun.preview.title });
    expect(within(preview).getByText(en.agentRun.states.building)).toBeInTheDocument();
    expect(within(preview).getByText(en.agentRun.preview.building)).toBeInTheDocument();
    expect(within(preview).getByText(/^1[0-9] s since the run started$/)).toBeInTheDocument();
    expect(screen.queryByTitle(en.agentRun.preview.frameTitle.replace("{app}", APP_TITLE))).toBeNull();

    const first = `/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/preview?v=1`;
    setRun({ ...RUN, status: "previewing", previewUrl: first });
    await emit("status", { seq: 5, status: "previewing" });
    const frame = await screen.findByTitle(en.agentRun.preview.frameTitle.replace("{app}", APP_TITLE));
    expect(frame).toHaveAttribute("src", first);

    // A second pass is a second URL: the frame is replaced, never left on the old document.
    const second = `/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/preview?v=2`;
    setRun({ ...RUN, status: "previewing", previewUrl: second });
    await emit("status", { seq: 9, status: "previewing" });
    await waitFor(() => {
      expect(
        screen.getByTitle(en.agentRun.preview.frameTitle.replace("{app}", APP_TITLE)),
      ).toHaveAttribute("src", second);
    });
    // The chat stands beside the preview with its composer open.
    expect(screen.getByLabelText(en.agentRun.conversation.placeholder)).toBeEnabled();
  });

  it("renders first frame and first version timings when present, and omits them when absent (T-0551)", async () => {
    const timingFirstFrame = (en.agentRun as { timing?: { firstFrame?: string } })?.timing?.firstFrame ?? "First frame";
    const timingFirstVersion = (en.agentRun as { timing?: { firstVersion?: string } })?.timing?.firstVersion ?? "First version";

    // Absent case: default RUN has neither firstFrameMs nor firstVersionMs
    const { view } = renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });

    expect(screen.queryByText(timingFirstFrame)).not.toBeInTheDocument();
    expect(screen.queryByText(timingFirstVersion)).not.toBeInTheDocument();

    view.unmount();

    // Present case: 12400 ms -> 12.4 s, 45600 ms -> 45.6 s
    renderRun({
      run: {
        ...RUN,
        firstFrameMs: 12_400,
        firstVersionMs: 45_600,
      },
    });
    await screen.findByRole("heading", { name: APP_TITLE });

    expect(screen.getByText(timingFirstFrame)).toBeInTheDocument();
    expect(screen.getByText(timingFirstVersion)).toBeInTheDocument();
    expect(screen.getByText("12.4 s")).toBeInTheDocument();
    expect(screen.getByText("45.6 s")).toBeInTheDocument();
  });

  it("cannot be stopped twice: a run that is over offers nothing", async () => {
    renderRun({ run: { ...RUN, status: "cancelled" } });
    await screen.findByRole("heading", { name: APP_TITLE });

    expect(screen.queryByRole("button", { name: en.agentRun.cancel })).toBeNull();
    expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeDisabled();
    expect(screen.getByText(en.agentRun.states.cancelled)).toBeInTheDocument();
  });

  it("names why a stop or a publish was refused", async () => {
    const user = userEvent.setup();
    renderRun({
      write: {
        status: 409,
        body: {
          type: "https://joinedcontext.com/errors/conflict",
          title: "Conflict",
          status: 409,
          detail: "the run is already cancelled",
        },
      },
    });
    await screen.findByRole("heading", { name: APP_TITLE });

    await user.click(screen.getByRole("button", { name: en.agentRun.cancel }));

    expect(await screen.findByText("the run is already cancelled")).toBeInTheDocument();
  });

  it("closes the stream when the page goes away", async () => {
    const { view } = renderRun();
    await screen.findByRole("heading", { name: APP_TITLE });
    const source = stream();

    view.unmount();

    expect(source.closed).toBe(true);
  });
});
