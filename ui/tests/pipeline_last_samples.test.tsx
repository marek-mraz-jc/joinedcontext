import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { LastSamples } from "../src/pages/pipelines/LastSamples";
import type { FlowNodeId } from "../src/pages/pipelines/PipelineFlow";

// T-3974: a selected node of a running pipeline shows the last messages through it, keyed by the
// runner's label (`step_{j}` for the j-th step), says when none passed, and says the messages are
// not available when the route answers 404 to a caller without read on the pipeline.
function answer(status: number, body: unknown): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      expect(url).toContain("/api/v1/projects/helsinki/pipelines/stations/samples");
      return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
    }),
  );
}

function open(node: FlowNodeId): void {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <LastSamples project="helsinki" pipeline="stations" node={node} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the last messages through a step", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the newest samples of the selected step, newest first", async () => {
    answer(200, {
      pipeline: "stations",
      withheld: false,
      nodes: {
        step_0: [
          { at: "2026-10-12T08:00:02Z", record: { n: 2 } },
          { at: "2026-10-12T08:00:01Z", record: { n: 1 } },
        ],
        input: [{ at: "2026-10-12T08:00:00Z", record: { n: 0, from: "input" } }],
      },
    });
    open("step-0");
    const items = await screen.findAllByRole("listitem");
    expect(items).toHaveLength(2);
    // T-5516: a record reads as rows, never as JSON text.
    expect(within(items[0]).getByText("n")).toBeInTheDocument();
    expect(within(items[0]).getByText("2")).toBeInTheDocument();
    expect(items[0].textContent).not.toContain("{\n");
    expect(screen.queryByText(/from/)).toBeNull();
  });

  it("says nothing passed yet when the step has no samples", async () => {
    answer(200, { pipeline: "stations", withheld: false, nodes: {} });
    open("step-0");
    expect(await screen.findByText("Nothing has passed through this step yet.")).toBeInTheDocument();
  });

  it("says a step before the mapping is withheld when the target hides attributes", async () => {
    answer(200, {
      pipeline: "stations",
      withheld: true,
      nodes: { output: [{ at: "2026-10-12T08:00:00Z", record: { n: 0 } }] },
    });
    open("step-0");
    expect(await screen.findByText(/^Withheld: the target endpoint hides attributes/)).toBeInTheDocument();
    expect(screen.queryByRole("listitem")).toBeNull();
  });

  it("says the messages are not available on a 404", async () => {
    answer(404, { title: "Not Found", status: 404 });
    open("step-0");
    expect(await screen.findByText("The last messages are not available.")).toBeInTheDocument();
  });
});
