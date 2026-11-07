/**
 * Live counts per node and minute on the canvas (T-3921, PL-68): the badge reads the last
 * minute, a wire into a node that failed in the last five minutes is red, a runner that does
 * not answer says so instead of painting zeros, a failing node shows its last error without the
 * record, and a hidden tab asks the Portal nothing.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  QueryClient,
  QueryClientProvider,
  focusManager,
} from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { NodeWindows } from "../src/pages/pipelines/PipelineFlow";
import {
  PipelineFlow,
  countersOfStats,
} from "../src/pages/pipelines/PipelineFlow";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: () => null,
}));
const { PipelineStudio } =
  await import("../src/pages/pipelines/PipelineStudio");

const form = {
  name: "hfp-vehicles",
  class: "resident",
  source: { dataSourceRef: "hfp" },
  processors: [
    { step: { processor: { dedupe: { cache: "c", key: "${! this.id }" } } } },
  ],
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-hfp",
} as PipelineForm;

const zero = { received: 0, sent: 0, errors: 0 };
const NODES: NodeWindows = {
  input: {
    "1m": { received: 120, sent: 120 },
    "5m": { received: 600, sent: 600 },
    "15m": zero,
  },
  step_0: {
    "1m": { received: 120, sent: 118, errors: 0 },
    "5m": { received: 600, sent: 590, errors: 10 },
    "15m": zero,
  },
  output: {
    "1m": { received: 118, sent: 118, errors: 0 },
    "5m": zero,
    "15m": zero,
  },
};

describe("countersOfStats", () => {
  it("reads the last minute's traffic and the last five minutes' errors", () => {
    expect(countersOfStats(NODES).step_0).toEqual({
      received: 120,
      sent: 118,
      errors: 10,
    });
    expect(countersOfStats(NODES).input.errors).toBeUndefined();
  });
});

describe("the live canvas", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  function renderFlow() {
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <PipelineFlow
            form={form}
            onChange={() => undefined}
            trace={null}
            live={countersOfStats(NODES)}
            selected={null}
            onSelect={() => undefined}
            dataSources={[]}
            endpoints={[]}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  it("shows counts per minute and the wire into a failing step red", () => {
    renderFlow();
    expect(screen.getByTestId("flow-node-source")).toHaveTextContent(
      "↓ 120/min",
    );
    expect(screen.getByTestId("flow-node-step-0")).toHaveAttribute(
      "data-state",
      "error",
    );
    expect(screen.getByTestId("flow-edge-source>step-0")).toHaveAttribute(
      "data-failing",
      "true",
    );
    expect(screen.getByTestId("flow-edge-step-0>output")).not.toHaveAttribute(
      "data-failing",
    );
  });
});

type Answer = { status: number; body: unknown };

function mockFetch(stats: () => Answer) {
  const json = ({ status, body }: Answer) =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
    );
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url;
    if (url.includes("/stats")) return json(stats());
    if (url.includes("/rejected")) {
      return json({
        status: 200,
        body: {
          total: 2,
          items: [
            {
              id: 2,
              at: "2026-10-12T08:00:00Z",
              record: { secret: "s3cr3t-value" },
              rule: "type",
              path: "type",
              message: "a newer refusal in the stage",
            },
            {
              id: 1,
              at: "2026-10-12T07:59:00Z",
              record: { secret: "s3cr3t-value" },
              rule: "step",
              path: "",
              message: "dedupe: cache unreachable",
              step: 0,
            },
          ],
        },
      });
    }
    return json({ status: 200, body: { kind: "List", items: [] } });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function renderStudio() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <PipelineStudio
          project="helsinki"
          draft={form}
          onChange={() => undefined}
          dataSources={[]}
          endpoints={[]}
          toManifest={(f) => ({
            kind: "Pipeline",
            metadata: { name: f.name },
            spec: f,
          })}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const statsCalls = (fetchMock: ReturnType<typeof mockFetch>) =>
  fetchMock.mock.calls.filter(([input]) =>
    String(input instanceof Request ? input.url : input).includes("/stats"),
  ).length;

describe("the studio's live counts", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
    focusManager.setFocused(undefined);
  });

  it("says the counts are unavailable when the runner does not answer, never zeros", async () => {
    mockFetch(() => ({
      status: 503,
      body: {
        status: 503,
        title: "Service Unavailable",
        detail: "the pipeline runner did not answer",
      },
    }));
    renderStudio();
    expect(
      await screen.findByTestId("flow-live-unavailable"),
    ).toHaveTextContent(en.pipelines.flow.liveUnavailable);
    expect(screen.queryByTestId("flow-live")).toBeNull();
    expect(screen.getByTestId("flow-node-step-0")).toHaveAttribute(
      "data-state",
      "idle",
    );
  });

  it("shows a failing step's last error, its message only", async () => {
    mockFetch(() => ({
      status: 200,
      body: {
        pipeline: "hfp-vehicles",
        scrapedAt: "t",
        coveredSeconds: 300,
        nodes: NODES,
      },
    }));
    renderStudio();
    await waitFor(() =>
      expect(screen.getByTestId("flow-node-step-0")).toHaveAttribute(
        "data-state",
        "error",
      ),
    );
    await userEvent.click(screen.getByTestId("flow-node-step-0"));
    const last = await screen.findByTestId("flow-last-error");
    expect(last).toHaveTextContent("dedupe: cache unreachable");
    expect(last).not.toHaveTextContent("a newer refusal in the stage");
    expect(document.body).not.toHaveTextContent("s3cr3t-value");
  });

  it("asks every five seconds while the tab is visible and not at all while it is hidden", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const fetchMock = mockFetch(() => ({
      status: 200,
      body: {
        pipeline: "hfp-vehicles",
        scrapedAt: "t",
        coveredSeconds: 0,
        nodes: NODES,
      },
    }));
    renderStudio();
    await waitFor(() => expect(statsCalls(fetchMock)).toBe(1));
    await act(() => vi.advanceTimersByTimeAsync(5_000));
    await waitFor(() => expect(statsCalls(fetchMock)).toBe(2));
    act(() => focusManager.setFocused(false));
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(statsCalls(fetchMock)).toBe(2);
  });
});
