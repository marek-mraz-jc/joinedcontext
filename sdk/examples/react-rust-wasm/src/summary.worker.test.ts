/** The worker's side (T-3327): one answer per question, by its id, or the module's failure in words. */
import { afterEach, describe, expect, it, vi } from "vitest";

const loading = vi.hoisted(() => ({ fail: false }));
vi.mock("../wasm/pkg/jc_wasm_example.js", () => ({
  default: () => (loading.fail ? Promise.reject(new Error("no WebAssembly here")) : Promise.resolve()),
  summarize: (values: Float64Array) => Float64Array.from([values.length, 1, 2, 1.5, 1.5]),
}));

afterEach(() => {
  vi.restoreAllMocks();
  vi.resetModules();
});

async function ask(values: number[]) {
  const posted = vi.spyOn(self, "postMessage").mockImplementation(() => undefined);
  await import("./summary.worker");
  self.dispatchEvent(new MessageEvent("message", { data: { id: 7, values: Float64Array.from(values) } }));
  await vi.waitFor(() => expect(posted).toHaveBeenCalled());
  // A worker module loaded by an earlier case still listens on the same `self`: every answer counts.
  return posted.mock.calls.map((call) => call[0]);
}

describe("the summary worker", () => {
  it("answers each question with the module's five numbers, by its id", async () => {
    expect(await ask([1, 2])).toEqual([{ id: 7, summary: [2, 1, 2, 1.5, 1.5] }]);
  });

  it("answers with the failure when the module cannot load", async () => {
    loading.fail = true;
    await vi.waitFor(async () => expect(await ask([1])).toContainEqual({ id: 7, error: "Error: no WebAssembly here" }));
    loading.fail = false;
  });
});
