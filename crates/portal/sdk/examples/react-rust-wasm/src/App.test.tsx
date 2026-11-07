/**
 * The example's screen (T-3416): a type and a number attribute picked from the model, the
 * entities read through the SDK, and the summary the WebAssembly module computes in its worker.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { initSync, summarize } from "../wasm/pkg/jc_wasm_example.js";
import module from "../wasm/pkg/jc_wasm_example_bg.wasm?inline";
import type { Row } from "@joinedcontext/sdk";
import App from "./App";
import type { Answer, Ask } from "./summary";

initSync({ module: Uint8Array.from(atob(module.slice(module.indexOf(",") + 1)), (c) => c.charCodeAt(0)) });

/** The worker, answered in place with the real module, or refusing when the test says so. */
class FakeWorker extends EventTarget {
  static refuse: string | null = null;
  postMessage({ id, values }: Ask): void {
    const data: Answer = FakeWorker.refuse ? { id, error: FakeWorker.refuse } : { id, summary: Array.from(summarize(values)) };
    queueMicrotask(() => this.dispatchEvent(new MessageEvent("message", { data })));
  }
}

const SCHEMA = {
  BikeHireDockingStation: { properties: { name: { type: "string" }, availableBikeNumber: { type: "integer" }, freeSlotNumber: { type: "number" } } },
  Alert: { properties: { name: { type: "string" } } },
};
const ROWS: Row[] = [
  { id: "urn:ngsi-ld:BikeHireDockingStation:1", type: "BikeHireDockingStation", availableBikeNumber: 4, freeSlotNumber: 2 },
  { id: "urn:ngsi-ld:BikeHireDockingStation:2", type: "BikeHireDockingStation", availableBikeNumber: 10, freeSlotNumber: 8 },
  { id: "urn:ngsi-ld:BikeHireDockingStation:3", type: "BikeHireDockingStation", availableBikeNumber: 1 },
];

function show(fixture: Parameters<typeof stubClient>[0] = { entities: ROWS, schema: SCHEMA }) {
  vi.stubGlobal("Worker", FakeWorker);
  render(
    <JcProvider client={stubClient(fixture)}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => {
  FakeWorker.refuse = null;
  vi.unstubAllGlobals();
});

describe("the summary screen", () => {
  it("summarizes the first number attribute of the first type that has one, and another when picked", async () => {
    show();
    expect(screen.getByRole("status")).toHaveTextContent("Loading…");
    const summary = await screen.findByTestId("summary");
    await waitFor(() => expect(within(summary).getAllByRole("definition")[0]).toHaveTextContent("3"));
    expect(screen.getByRole("combobox", { name: "Type" })).toHaveValue("BikeHireDockingStation");
    expect(within(summary).getByText("median").nextSibling).toHaveTextContent("4");
    fireEvent.change(screen.getByRole("combobox", { name: "Attribute" }), { target: { value: "freeSlotNumber" } });
    await waitFor(() => expect(within(screen.getByTestId("summary")).getAllByRole("definition")[0]).toHaveTextContent("2"));
    fireEvent.change(screen.getByRole("combobox", { name: "Type" }), { target: { value: "BikeHireDockingStation" } });
    expect(screen.getByRole("combobox", { name: "Type" })).toHaveValue("BikeHireDockingStation");
  });

  it("says a summary of no values is a count of none", async () => {
    show({ entities: [{ id: "urn:ngsi-ld:BikeHireDockingStation:9", type: "BikeHireDockingStation" }], schema: SCHEMA });
    const summary = await screen.findByTestId("summary");
    expect(within(summary).getAllByRole("definition")).toHaveLength(1);
    expect(within(summary).getByRole("definition")).toHaveTextContent("0");
  });

  it("says what the worker could not compute", async () => {
    FakeWorker.refuse = "the module failed to load";
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent("the module failed to load");
  });

  it("says when no type has a number, and when the endpoint cannot be read", async () => {
    show({ entities: [], schema: { Alert: SCHEMA.Alert } });
    expect(await screen.findByText("No type of this endpoint has a number attribute.")).toBeInTheDocument();
  });

  it("says why the model could not be read", async () => {
    show({ entities: ROWS, schema: SCHEMA, refuse: (request) => (request.path.includes("schema") ? { status: 503, body: { title: "Service Unavailable", status: 503 } } : null) });
    expect(await screen.findByRole("alert")).toHaveTextContent(/Service Unavailable|503/);
  });
});
