import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ActivityFeed } from "../src/components/ActivityFeed";
import { mergeActivity, objectOf, TAIL_WINDOW } from "../src/api/activity";

// The links are the router's; its address is what a test reads (T-6230).
vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, params, children, ...rest }: { to: string; params: Record<string, string>; children: React.ReactNode }) => (
    <a href={to.replace(/\$(\w+)/g, (_, key: string) => params[key])} {...rest}>
      {children}
    </a>
  ),
}));
import { summarise } from "../src/components/ActivitySummary";
import type { ActivityEvent } from "../src/api/activity";

const PROJECT = "helsinki";

function event(overrides: Partial<ActivityEvent> = {}): ActivityEvent {
  return {
    time: "2026-09-16T16:21:03Z",
    project: PROJECT,
    space: "air-quality",
    kind: "access.denied",
    source: "gateway",
    summary: "An anonymous caller was refused a write.",
    severity: "warning",
    details: { object: "endpoints/public-air" },
    ...overrides,
  } as ActivityEvent;
}

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, ((event: MessageEvent) => void)[]> = {};

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(name: string, handler: (event: MessageEvent) => void) {
    (this.listeners[name] ||= []).push(handler);
  }

  removeEventListener(name: string, handler: (event: MessageEvent) => void) {
    this.listeners[name] = (this.listeners[name] || []).filter(
      (h) => h !== handler,
    );
  }

  close() {
    MockEventSource.instances = MockEventSource.instances.filter(
      (i) => i !== this,
    );
  }

  emit(name: string, data: unknown) {
    for (const handler of this.listeners[name] || []) {
      handler({ data: JSON.stringify(data) } as MessageEvent);
    }
  }
}

/** The rows the feed shows, head excluded; the skeleton renders none of them. */
async function rows() {
  return await waitFor(() => {
    const table = screen.getByRole("table");
    const found = within(table).getAllByRole("row").slice(1);
    expect(found.length).toBeGreaterThan(0);
    return found;
  });
}

function renderFeed(
  items: ActivityEvent[],
  props: Record<string, unknown> = {},
) {
  const urls: string[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = input instanceof Request ? input.url : String(input);
    urls.push(url);
    const body = url.includes("/activity")
      ? {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "ActivityList",
          items,
        }
      : url.includes("/api/v1/connectors")
        ? { items: (props.connectors as unknown[] | undefined) ?? [] }
        : {};
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ActivityFeed project={PROJECT} {...props} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return urls;
}

describe("the activity feed (UI-31)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    MockEventSource.instances = [];
    vi.stubGlobal("EventSource", MockEventSource);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows what happened, in the order the API sent it, with the object it belongs to", async () => {
    renderFeed([
      event({ time: "2026-09-16T16:25:00Z", summary: "The newer one." }),
      event({ time: "2026-09-16T16:21:03Z", summary: "The older one." }),
    ]);

    const shown = await rows();
    expect(shown).toHaveLength(2);
    expect(shown[0]).toHaveTextContent("The newer one.");
    expect(
      within(shown[0]).getByRole("link", { name: "endpoints/public-air" }),
    ).toHaveAttribute("href", "/projects/helsinki/endpoints/public-air");
  });

  it("gives the sentence the row's width on a phone: time, kind and source fold into one line above it (T-4165)", async () => {
    renderFeed([event({ time: "2026-09-16T16:25:00Z", summary: "The newer one." })]);
    const [row] = await rows();
    const cells = within(row).getAllByRole("cell");
    // Only the sentence's column is there below sm; the other three show from sm up.
    expect(cells.filter((cell) => !cell.className.includes("hidden sm:table-cell"))).toHaveLength(1);
    const sentence = cells.find((cell) => !cell.className.includes("hidden"));
    const phoneLine = sentence?.querySelector(".sm\\:hidden");
    expect(phoneLine).not.toBeNull();
    expect(phoneLine?.textContent).toContain(within(row).getByTestId("activity-kind").textContent ?? "");
    for (const header of within(screen.getByRole("table")).getAllByRole("columnheader")) {
      if (header.textContent !== en.activity.summary) expect(header).toHaveClass("hidden", "sm:table-cell");
    }
  });

  it("says a failed publication's sentence and keeps its problem body behind a disclosure (T-4200)", async () => {
    const reason =
      'the dataset is unchanged, its sheet was not refreshed: GET https://portal.example/api/endpoint/x/file.csv: 503 {"type":"https://joinedcontext.com/errors/broker-failure","status":503}';
    renderFeed([
      event({
        kind: "catalogue.published",
        source: "ckan",
        severity: "error",
        summary: `Endpoint helsinki-events was not published: ${reason}`,
        details: { reason, object: "endpoints/helsinki-events" },
      }),
      // A summary that does not end with its reason is shown as written.
      event({
        summary: "Endpoint helsinki-all is dataset helsinki-all.",
        details: { reason: "elsewhere" },
      }),
    ]);

    const [failed, other] = await rows();
    expect(within(failed).getByTestId("activity-sentence")).toHaveTextContent(
      /^Endpoint helsinki-events was not published\.$/,
    );
    const disclosure = within(failed)
      .getByText(en.activity.technical)
      .closest("details")!;
    expect(disclosure).not.toHaveAttribute("open");
    expect(within(failed).getByTestId("activity-reason")).toHaveTextContent(
      reason,
    );
    expect(other).toHaveTextContent(
      "Endpoint helsinki-all is dataset helsinki-all.",
    );
    expect(within(other).queryByTestId("activity-reason")).toBeNull();
  });

  it("links a merged change to its own page and links nothing that is not a change id (T-3292)", async () => {
    renderFeed([
      event({
        kind: "change.merged",
        source: "portal",
        severity: "info",
        summary:
          "Change chg-0000001c merged: create Endpoint public-air, approved by eva.",
        details: { change: "chg-0000001c", object: "endpoints/public-air" },
      }),
      event({
        kind: "change.merged",
        source: "portal",
        severity: "info",
        summary: "A merge naming no change id.",
        details: { change: "javascript:alert(1)" },
      }),
    ]);
    const shown = await rows();
    expect(
      within(shown[0]).getByRole("link", { name: "chg-0000001c" }),
    ).toHaveAttribute("href", "/projects/helsinki/approvals/chg-0000001c");
    expect(
      within(shown[0]).getByRole("link", { name: "endpoints/public-air" }),
    ).toBeInTheDocument();
    expect(within(shown[1]).queryByRole("link")).toBeNull();
  });

  it("shows a rejected change with its rejecter, its reason and a link to it (T-5613)", async () => {
    renderFeed([
      event({
        kind: "change.rejected",
        source: "portal",
        severity: "info",
        summary:
          "Change chg-0000001d rejected: create Endpoint public-air, by eva: The notes are private",
        details: {
          change: "chg-0000001d",
          object: "endpoints/public-air",
          rejectedBy: "eva",
          reason: "The notes are private",
        },
      }),
    ]);
    const [row] = await rows();
    expect(within(row).getByTestId("activity-kind")).toHaveTextContent(
      en.activity.kinds.change.rejected,
    );
    // The approver's words stay in the row, not behind the technical disclosure.
    expect(within(row).getByTestId("activity-sentence")).toHaveTextContent(
      "Change chg-0000001d rejected: create Endpoint public-air, by eva: The notes are private",
    );
    expect(within(row).queryByTestId("activity-reason")).toBeNull();
    expect(
      within(row).getByRole("link", { name: "chg-0000001d" }),
    ).toHaveAttribute("href", "/projects/helsinki/approvals/chg-0000001d");
  });

  it("puts a filter into the request the list makes", async () => {
    const urls = renderFeed([event()]);
    await rows();

    await userEvent.selectOptions(
      screen.getByLabelText(en.activity.filter.severity),
      "error",
    );
    await waitFor(() =>
      expect(urls.some((url) => url.includes("severity=error"))).toBe(true),
    );
  });

  // T-5973: the connector filter offers the reader's own connectors, which the server lists, and
  // narrows the request to the one picked; without any it says where to make one.
  it("filters by one of the reader's connectors", async () => {
    const urls = renderFeed([event()], {
      connectors: [{ id: "abcdefghijklmnopqrstuvwxyz", name: "doprava-in-claude" }],
    });
    await rows();
    const select = screen.getByLabelText(en.activity.filter.connector);
    await screen.findByRole("option", { name: "doprava-in-claude" });
    expect(screen.getByText(en.activity.filter.connectorHint)).toBeInTheDocument();
    await userEvent.selectOptions(select, "abcdefghijklmnopqrstuvwxyz");
    await waitFor(() =>
      expect(
        urls.some((url) => url.includes("/activity?") && url.includes("connector=abcdefghijklmnopqrstuvwxyz")),
      ).toBe(true),
    );
  });

  it("offers no connector filter when the reader has none", async () => {
    renderFeed([event()]);
    await rows();
    expect(screen.getByLabelText(en.activity.filter.severity)).toBeInTheDocument();
    expect(screen.queryByLabelText(en.activity.filter.connector)).toBeNull();
  });

  it("follows the tail and puts an arriving event at the top", async () => {
    renderFeed([event({ summary: "What the page loaded." })]);
    await rows();

    const source = MockEventSource.instances.at(-1);
    expect(source, "the tail is on by default").toBeDefined();
    source?.emit(
      "pipeline.error",
      event({
        time: "2026-09-16T17:00:00Z",
        kind: "pipeline.error",
        severity: "error",
        summary: "A mapping dropped a reading.",
      }),
    );

    await waitFor(() => {
      const shown = within(screen.getByRole("table"))
        .getAllByRole("row")
        .slice(1);
      expect(shown[0]).toHaveTextContent("A mapping dropped a reading.");
    });
    // Only an error interrupts a reader; the live region says nothing else.
    await waitFor(() =>
      expect(screen.getByTestId("activity-announce")).toHaveTextContent(
        "1 new error",
      ),
    );
  });

  it("stops following when the tail is switched off", async () => {
    renderFeed([event()]);
    await rows();
    expect(MockEventSource.instances).toHaveLength(1);

    await userEvent.click(
      screen.getByRole("switch", { name: en.activity.tail }),
    );
    await waitFor(() => expect(MockEventSource.instances).toHaveLength(0));
  });

  it("an object panel offers no filters and no tail toggle", async () => {
    renderFeed([event()], {
      compact: true,
      fixed: { object: "endpoints/public-air" },
    });
    await rows();

    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.queryByLabelText(en.activity.filter.kind)).toBeNull();
  });

  it("says so when nothing has happened", async () => {
    renderFeed([]);
    expect(await screen.findByText(en.activity.empty)).toBeInTheDocument();
  });
});

describe("the tail's reducer", () => {
  it("keeps one copy of an event the page already showed", () => {
    const known = [event({ summary: "One." })];
    expect(mergeActivity(known, [event({ summary: "One." })])).toBe(known);
  });

  it("orders newest first and never grows past the window", () => {
    const many = Array.from({ length: TAIL_WINDOW + 20 }, (_, i) =>
      event({
        time: `2026-09-16T${String(i % 24).padStart(2, "0")}:00:0${i % 10}Z`,
        summary: `n${i}`,
      }),
    );
    const merged = mergeActivity([], many);
    expect(merged).toHaveLength(TAIL_WINDOW);
    expect(merged[0].time >= merged[1].time).toBe(true);
  });

  it("reads the object an event belongs to, and nothing that is not one", () => {
    expect(objectOf(event())).toBe("endpoints/public-air");
    expect(
      objectOf(event({ details: { object: "no-slash" } })),
    ).toBeUndefined();
    expect(objectOf(event({ details: {} }))).toBeUndefined();
  });
});

describe("the last hour strip", () => {
  it("counts what an event counted, and one for an event that counted nothing", () => {
    expect(
      summarise([
        event({ kind: "pipeline.throughput", details: { count: 1200 } }),
        event({ kind: "pipeline.throughput", details: { count: 300 } }),
        event({ kind: "access.denied" }),
        event({ kind: "access.denied" }),
        event({ kind: "config.applied" }),
        event({ kind: "change.merged" }),
        event({ kind: "mcp.tool" }),
      ]),
    ).toEqual({
      messages: 1500,
      changes: 2,
      requests: 0,
      forwards: 0,
      denials: 2,
    });
  });

  it("ignores a count that is not a number", () => {
    expect(
      summarise([
        event({ kind: "endpoint.traffic", details: { count: "many" } }),
      ]).requests,
    ).toBe(1);
  });
});

/**
 * The tail is the one place the Portal renders something it did not type: `subscribeActivity`
 * casts the frame's JSON to `ActivityEvent`. A frame whose `time` is absent or unreadable used
 * to reach `Intl.DateTimeFormat.format(new Date(undefined))`, which throws `RangeError: Invalid
 * time value` — and the only error boundary is the one around the whole app, so one bad frame
 * blanked the Portal (T-2427).
 */
describe("a frame the tail cannot read", () => {
  const BAD: Record<string, unknown>[] = [
    { ...event(), time: undefined },
    { ...event(), time: "the day before yesterday" },
    { ...event(), severity: undefined },
    { ...event(), summary: 7 },
    "a bare string",
    null,
  ] as unknown as Record<string, unknown>[];

  it("is dropped instead of taking the page down with it", async () => {
    renderFeed([event({ summary: "What the page loaded." })]);
    await rows();
    const source = MockEventSource.instances.at(-1);

    for (const frame of BAD) {
      source?.emit("message", frame);
    }

    const shown = await rows();
    expect(shown, "nothing unreadable was added").toHaveLength(1);
    expect(shown[0]).toHaveTextContent("What the page loaded.");
    // And it is not counted as arrived: the reader is told about events they can see.
    expect(screen.getByTestId("activity-arrived")).toHaveTextContent("0");
  });

  it("still takes the next readable frame", async () => {
    renderFeed([event({ summary: "What the page loaded." })]);
    await rows();
    const source = MockEventSource.instances.at(-1);

    source?.emit("message", { ...event(), time: "" });
    source?.emit(
      "pipeline.error",
      event({
        time: "2026-09-16T17:00:00Z",
        kind: "pipeline.error",
        severity: "error",
        summary: "A mapping dropped a reading.",
      }),
    );

    await waitFor(() => {
      const shown = within(screen.getByRole("table"))
        .getAllByRole("row")
        .slice(1);
      expect(shown[0]).toHaveTextContent("A mapping dropped a reading.");
    });
  });

  it("keeps an event whose missing field the row never draws", async () => {
    renderFeed([event({ summary: "What the page loaded." })]);
    await rows();
    const source = MockEventSource.instances.at(-1);

    const { project: _project, ...noProject } = event({
      summary: "Drawn all the same.",
    });
    source?.emit("message", noProject);

    await waitFor(() => {
      const shown = within(screen.getByRole("table"))
        .getAllByRole("row")
        .slice(1);
      expect(shown[0]).toHaveTextContent("Drawn all the same.");
    });
  });

  it("keeps a page the API answered with a broken row readable", () => {
    const merged = mergeActivity(
      [
        { ...event(), time: "yesterday" } as ActivityEvent,
        event({ summary: "Readable." }),
      ],
      [],
    );
    expect(merged.map((e) => e.summary)).toEqual(["Readable."]);
  });
});
