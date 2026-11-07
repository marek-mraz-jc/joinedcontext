import { describe, expect, it } from "vitest";
import { initSync, summarize } from "../wasm/pkg/jc_wasm_example.js";
import module from "../wasm/pkg/jc_wasm_example_bg.wasm?inline";
import type { Answer, Ask, Port } from "./summary";
import { numericAttributes, toSummary, valuesOf, workerSummary } from "./summary";

// The module the build lane compiled (wasm/pkg), its bytes as Vite inlines them.
initSync({ module: Uint8Array.from(atob(module.slice(module.indexOf(",") + 1)), (c) => c.charCodeAt(0)) });

describe("the summary of one attribute", () => {
  const rows = [
    { id: "urn:ngsi-ld:BikeHireDockingStation:1", type: "BikeHireDockingStation", availableBikeNumber: 4 },
    { id: "urn:ngsi-ld:BikeHireDockingStation:2", type: "BikeHireDockingStation", availableBikeNumber: 10 },
    { id: "urn:ngsi-ld:BikeHireDockingStation:3", type: "BikeHireDockingStation", availableBikeNumber: null },
    { id: "urn:ngsi-ld:BikeHireDockingStation:4", type: "BikeHireDockingStation", availableBikeNumber: "7" },
    { id: "urn:ngsi-ld:BikeHireDockingStation:5", type: "BikeHireDockingStation", availableBikeNumber: 1 },
  ];

  it("is computed by the Rust module from the numbers the rows hold", () => {
    expect(toSummary(summarize(valuesOf(rows, "availableBikeNumber")))).toEqual({ count: 3, min: 1, max: 10, mean: 5, median: 4 });
    expect(toSummary(summarize(valuesOf(rows, "missing"))).count).toBe(0);
  });

  it("offers the attributes the model says are numbers", () => {
    const schema = { S: { properties: { name: { type: "string" }, bikes: { type: ["integer", "null"] }, rate: { type: "number" } } } };
    expect(numericAttributes(schema, "S")).toEqual(["bikes", "rate"]);
    expect(numericAttributes(schema, "Other")).toEqual([]);
  });

  it("matches each worker answer to its own question", async () => {
    const listeners = new Set<(event: MessageEvent<Answer>) => void>();
    const asked: Ask[] = [];
    const worker: Port = {
      postMessage: (ask) => {
        asked.push(ask);
      },
      addEventListener: (_, fn) => {
        listeners.add(fn);
      },
      removeEventListener: (_, fn) => {
        listeners.delete(fn);
      },
    };
    const ask = workerSummary(worker);
    const first = ask(Float64Array.from([1, 2]));
    const second = ask(Float64Array.from([5]));
    const answer = (data: Answer) => [...listeners].forEach((fn) => fn(new MessageEvent("message", { data })));
    answer({ id: asked[1].id, summary: Array.from(summarize(asked[1].values)) });
    answer({ id: asked[0].id, error: "the module failed" });
    await expect(second).resolves.toMatchObject({ count: 1, mean: 5 });
    await expect(first).rejects.toThrow("the module failed");
    expect(listeners.size).toBe(0);
  });
});
