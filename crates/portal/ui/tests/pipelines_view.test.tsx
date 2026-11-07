import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { executionClass } from "../src/routes/PipelinesPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const PIPELINES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "aq-mqtt-ingest", namespace: "banskabystrica" },
      spec: {
        class: "resident",
        secretRefs: [
          // A manifest may never carry a value; if one is smuggled in, no view renders it.
          { name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD", value: "hunter2" },
        ],
      },
      status: {
        phase: "Live",
        sourceUrl:
          "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/pipelines/aq-mqtt-ingest/pipeline.yaml",
      },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "parking-daily", namespace: "banskabystrica" },
      spec: { class: "auto", period: "1h", enabled: false, expiry: { after: "14d", types: ["Vehicle"] } },
      status: { phase: "Live" },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "air-hourly", namespace: "banskabystrica" },
      spec: {
        class: "auto",
        schedule: "0 * * * *",
        source: { dataSourceRef: { kind: "DataSource", name: "air-feed" } },
      },
      status: { phase: "Live" },
    },
  ],
};

const METRICS = {
  pipeline: "aq-mqtt-ingest",
  scrapedAt: "2026-09-06T16:20:11Z",
  received: 128401,
  sent: 128390,
  errors: 3,
  latencyP99Ms: 42.5,
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

/** What the dry run before a write answers: green unless a test asks for a red check (PF-57). */
const GREEN = { valid: true, verdict: { ok: true, findings: [] } };

function renderPipelines(
  metrics: unknown = METRICS,
  metricsStatus = 200,
  pipelines: unknown = PIPELINES,
  check: unknown = GREEN,
) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (request.method !== "GET") {
      // A write is checked on the same route and verb first (T-2264); only the real one answers
      // with the Change.
      return new URL(request.url).searchParams.get("dryRun") === "All"
        ? json(check)
        : json(CHANGE, 202);
    }
    if (path.endsWith("/metrics")) {
      return json(metrics, metricsStatus);
    }
    if (path.endsWith("/pipelines")) {
      return json(pipelines);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
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

const rowOf = async (name: string) =>
  (await screen.findByText(name)).closest("tr") as HTMLElement;

describe("pipelines view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("classifies by the 30-second rule when the manifest says auto", () => {
    expect(executionClass({ class: "resident", period: "1h" })).toBe("resident");
    expect(executionClass({ class: "scheduled" })).toBe("scheduled");
    expect(executionClass({ class: "auto" })).toBe("resident");
    expect(executionClass({ class: "auto", period: "250ms" })).toBe("resident");
    expect(executionClass({ class: "auto", period: "29s" })).toBe("resident");
    expect(executionClass({ class: "auto", period: "30s" })).toBe("scheduled");
    expect(executionClass({ class: "auto", period: "5m" })).toBe("scheduled");
    expect(executionClass({ class: "auto", schedule: "10 0 * * *" })).toBe("scheduled");
  });

  /// T-1058: an empty list is where a person decides what to do next.
  it("says the two ways in when the project has no pipeline yet", async () => {
    renderPipelines(METRICS, 200, {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [],
    });

    expect(await screen.findByText(en.pipelines.empty)).toBeInTheDocument();
    expect(screen.getByText(en.pipelines.emptyHint)).toBeInTheDocument();
    // The way in is a button, in the empty state as well as in the header.
    expect(screen.getAllByRole("button", { name: en.pipelines.add }).length).toBeGreaterThan(1);
  });

  it("shows the execution class of every pipeline", async () => {
    renderPipelines();

    expect(within(await rowOf("aq-mqtt-ingest")).getByText(en.pipelines.class.resident))
      .toBeInTheDocument();
    expect(within(await rowOf("parking-daily")).getByText(en.pipelines.class.scheduled))
      .toBeInTheDocument();
  });

  /// PL-65: a pipeline that removes stale entities says so in its row; one that does not, nothing.
  it("says which pipelines remove the entities their source stopped sending", async () => {
    renderPipelines();

    const sentence = en.pipelines.expiryRemoves.replace("{window}", "14 days");
    expect(within(await rowOf("parking-daily")).getByText(sentence)).toBeInTheDocument();
    expect(within(await rowOf("aq-mqtt-ingest")).queryByText(/are removed/)).toBeNull();
  });

  it("says in the row that the runner refused the sweep, with its reason", async () => {
    // The `.expiry` stream failing left the pipeline writing and nothing removed; only the
    // reconciler's condition knew (PL-64, T-3091).
    const refused = structuredClone(PIPELINES);
    refused.items[1].status = {
      phase: "Live",
      conditions: [{ type: "ExpirySweep", status: "False", reason: "RunnerRefused", message: "lint: unknown field" }],
    } as never;
    renderPipelines(METRICS, 200, refused);
    expect(within(await rowOf("parking-daily")).getByTestId("pipeline-expiry-refused")).toHaveTextContent(
      "Nothing is removed: the runner refused the sweep: lint: unknown field",
    );
    expect(within(await rowOf("aq-mqtt-ingest")).queryByTestId("pipeline-expiry-refused")).toBeNull();
  });

  it("shows a resident stream's totals since its runner started, its errors a link to its runs", async () => {
    renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    expect(await within(row).findByText(/128,401 received/)).toBeInTheDocument();
    const errors = within(row).getByRole("link", { name: "3 errors" });
    expect(errors).toHaveAttribute("href", "/projects/banskabystrica/pipelines/aq-mqtt-ingest#runs");
    // The p99 latency read as "ms p99 10,000.0" with no word of what it was (T-4817): the page of
    // the pipeline is where a person reads its stream.
    expect(within(row).queryByText("42.5")).toBeNull();
  });

  it("never shows a secret value (PL-17)", async () => {
    renderPipelines();

    await rowOf("aq-mqtt-ingest");
    expect(screen.queryByText(/hunter2/)).not.toBeInTheDocument();
  });

  it("keeps a runner that is down out of the pipeline's status", async () => {
    renderPipelines({ status: 503, title: "Service Unavailable" }, 503);

    const row = await rowOf("aq-mqtt-ingest");
    expect(await within(row).findByText(en.pipelines.lastRun.never)).toBeInTheDocument();
    expect(within(row).queryByText(/received/)).toBeNull();
    // The manifest is still Live: a runner that is down is not a broken pipeline.
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();
  });

  it("does not poll a paused pipeline and offers to start it again", async () => {
    const fetchMock = renderPipelines();

    const row = await rowOf("parking-daily");
    expect(within(row).getByText(en.phase.paused)).toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some((call) =>
        new URL((call[0] as Request).url).pathname.includes("parking-daily/metrics"),
      ),
    ).toBe(false);

    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.resume }));

    await waitFor(() => {
      const write = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((request) => request.method === "PUT");
      expect(write?.url).toContain("/pipelines/parking-daily");
    });
  });

  // T-2759: the list said "Live" beside "Paused" beside a Pause button, for a scheduled pipeline
  // that was running and for a paused one that was not.
  it("says when a scheduled pipeline last ran, and polls no stream for it", async () => {
    const fetchMock = renderPipelines();
    const row = await rowOf("air-hourly");
    expect(await within(row).findByText(en.pipelines.lastRun.never)).toBeInTheDocument();
    expect(within(row).queryByText(en.phase.paused)).toBeNull();
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: en.pipelines.pause })).toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some((call) =>
        new URL((call[0] as Request).url).pathname.includes("air-hourly/metrics"),
      ),
    ).toBe(false);
  });

  it("says a paused pipeline is paused, not live", async () => {
    renderPipelines();
    const row = await rowOf("parking-daily");
    expect(within(row).getByText(en.phase.paused)).toBeInTheDocument();
    expect(within(row).queryByText(en.phase.live)).toBeNull();
  });

  it("says a Live stream that lands nothing writes nothing, with the reason (T-2967)", async () => {
    const stalled =
      "the stream took 21000 record(s) in over the last 10 minute(s) and sent, rejected or failed none of them";
    const pipelines = {
      ...PIPELINES,
      items: PIPELINES.items.map((item) =>
        item.metadata.name === "aq-mqtt-ingest"
          ? {
              ...item,
              status: {
                ...item.status,
                conditions: [{ type: "StreamWriting", status: "False", reason: "Stalled", message: stalled }],
              },
            }
          : item,
      ),
    };
    renderPipelines(METRICS, 200, pipelines);
    const row = await rowOf("aq-mqtt-ingest");
    // Still Live, because it is deployed; the badge is what says nothing lands.
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();
    const badge = within(row).getByText(en.pipelines.notWriting);
    expect(badge.closest("[title]")).toHaveAttribute("title", stalled);
    expect(within(row).getByText(`: ${stalled}`)).toBeInTheDocument();
    // A stream that writes carries no such badge.
    expect(within(await rowOf("air-hourly")).queryByText(en.pipelines.notWriting)).toBeNull();
  });

  // T-4817: "Held by its data source …" took a column on every row; the source's name says it.
  it("names the data source a pipeline reads under its title", async () => {
    renderPipelines();
    const row = await rowOf("air-hourly");
    expect(within(row).getByText("air-feed")).toBeInTheDocument();
    expect(within(row).queryByText(/Held by its data source/)).toBeNull();
  });

  it("proposes a change instead of stopping a running pipeline directly (CC-35)", async () => {
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    const fetchMock = renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    const write = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((r) => r.method === "PUT");
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(write.headers.get("x-csrf-token")).toBe("csrf-token-value");
    const body = JSON.parse(await write.clone().text()) as {
      spec: { enabled: boolean };
      status?: unknown;
    };
    expect(body.spec.enabled).toBe(false);
    expect(body.status).toBeUndefined();
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
  });

  it("checks the manifest on the same route before it writes it (PF-57, T-2264)", async () => {
    const fetchMock = renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    // Two requests, in this order: the dry run that records the verdict, then the write. Without
    // the check the verdict gate refuses the write with "The manifest has not been checked" and
    // the click does nothing at all.
    const writes = await waitFor(() => {
      const requests = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .filter((request) => request.method === "PUT");
      expect(requests).toHaveLength(2);
      return requests;
    });
    expect(new URL(writes[0].url).searchParams.get("dryRun")).toBe("All");
    expect(new URL(writes[1].url).searchParams.get("dryRun")).toBe(null);
    // The same manifest both times: a check of something else proves nothing about what lands.
    expect(await writes[0].clone().text()).toBe(await writes[1].clone().text());
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
  });

  it("writes nothing when the check is red, and says what it found", async () => {
    const fetchMock = renderPipelines(METRICS, 200, PIPELINES, {
      valid: false,
      verdict: { ok: false, findings: [{ message: "the runner has no room for another stream" }] },
    });

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    expect(await screen.findByText(/the runner has no room for another stream/)).toBeInTheDocument();
    const writes = fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .filter((request) => request.method === "PUT");
    expect(writes).toHaveLength(1);
    expect(new URL(writes[0].url).searchParams.get("dryRun")).toBe("All");
  });

  it("checks and writes inside the copy a person is working on, never in the project", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines?workspace=trial");
    const fetchMock = renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    const writes = await waitFor(() => {
      const requests = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .filter((request) => request.method === "PUT");
      expect(requests).toHaveLength(2);
      return requests;
    });
    // Both of them: a check recorded against the project would be a verdict for another manifest,
    // and the gate would refuse the write it was supposed to let through (CC-76).
    for (const request of writes) {
      expect(new URL(request.url).searchParams.get("workspace")).toBe("trial");
    }
  });
});
