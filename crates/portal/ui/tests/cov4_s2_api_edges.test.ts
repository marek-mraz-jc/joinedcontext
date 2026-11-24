// T-7706: api edges: a transport failure that is no ApiError reaches the caller, a sample address that is no URL
// goes to the runner as it is, and the step-up announcements and a refusal whose body is not JSON.
import { renderHook, act } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { doorAction } from "../src/api/odata";
import { callOperation } from "../src/api/operations";
import { testPipeline } from "../src/api/pipelineTest";
import { announceStepUp, clearStepUp, createStepUpMiddleware, useStepUp } from "../src/api/stepUp";

vi.mock("../src/api/odata", () => ({ doorAction: vi.fn() }));
const door = vi.mocked(doorAction);

afterEach(() => {
  vi.unstubAllGlobals();
  door.mockReset();
});

describe("transport failures", () => {
  it("lets a network error out of callOperation and testPipeline instead of turning it into a status", async () => {
    door.mockRejectedValue(new TypeError("network down"));
    await expect(callOperation("helsinki", "jc_x", {})).rejects.toThrow("network down");
    await expect(testPipeline("helsinki", {}, { inline: "{}" } as never)).rejects.toThrow("network down");
  });
});

describe("testPipeline sample", () => {
  it("sends a sample whose url is not an address to the runner untouched", async () => {
    door.mockResolvedValue({ ok: true, errors: [] });
    await testPipeline("helsinki", { kind: "Pipeline" }, { url: "not a url" } as never);
    expect(JSON.stringify(door.mock.calls[0])).toContain("not a url");
  });
});

describe("step-up", () => {
  it("ignores an announcement nobody listens to, and the same one twice", () => {
    clearStepUp();
    announceStepUp("/login?step_up=true");
    const seen = renderHook(() => useStepUp());
    expect(seen.result.current).toBeNull();
    act(() => announceStepUp("/login?step_up=true"));
    expect(seen.result.current).toBe("/login?step_up=true");
    act(() => announceStepUp("/login?step_up=true"));
    expect(seen.result.current).toBe("/login?step_up=true");
    act(() => clearStepUp());
    expect(seen.result.current).toBeNull();
  });

  it("raises nothing for a 403 whose body is not JSON", async () => {
    const raise = vi.fn();
    const middleware = createStepUpMiddleware(raise);
    const response = new Response("<html>forbidden</html>", { status: 403 });
    const onResponse = middleware.onResponse as (options: { response: Response }) => Promise<Response>;
    expect(await onResponse({ response })).toBe(response);
    expect(raise).not.toHaveBeenCalled();
  });
});
