/**
 * The preview bridge as AgentRunPage mounts it (SDK-18, SDK-27): only the frame the page created
 * is heard, its error reports reach the run once each and at most MAX_ERRORS of them, a refused
 * message is counted, a ready frame of a live run is asked to observe, and unmounting stops it.
 */
import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RefObject } from "react";

const doorAction = vi.fn(async (..._args: unknown[]) => ({}));
vi.mock("../src/api/odata", () => ({ doorAction: (...args: unknown[]) => doorAction(...args) }));

import { handleBridgeMessage, MAX_ERRORS, observationRelay, usePreviewBridge } from "../src/pages/apps/previewBridge";

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const OTHER = "a7m2qz4tv6xh3n5jb2ryd3wcfa";
const run = { id: "r1", project: "helsinki", endpointSlug: SLUG, endpoints: [{ slug: SLUG }, { slug: OTHER }], dataNeeds: [], previewUrl: "/api/v1/projects/helsinki/agent-runs/r1/preview?v=4" };

let iframe: HTMLIFrameElement;
let ref: RefObject<HTMLIFrameElement | null>;
let posted: ReturnType<typeof vi.fn>;

beforeEach(() => {
  doorAction.mockClear();
  iframe = document.createElement("iframe");
  document.body.append(iframe);
  ref = { current: iframe };
  posted = vi.fn();
  (iframe.contentWindow as Window).postMessage = posted as unknown as Window["postMessage"];
});

afterEach(() => {
  iframe.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function message(data: unknown, source: unknown = iframe.contentWindow) {
  window.dispatchEvent(new MessageEvent("message", { data, source: source as Window }));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const error = (message: string) => ({ kind: "jc-error", message, file: "src/App.tsx", line: 3 });

describe("usePreviewBridge", () => {
  it("reports each distinct frame error once, at most MAX_ERRORS, and nothing from another window", async () => {
    renderHook(() => usePreviewBridge(ref, { ...run, previewUrl: "/apps/x/r2/" }, true));
    await message(error("boom"));
    await message(error("boom"));
    expect(doorAction).toHaveBeenCalledTimes(1);
    expect(doorAction).toHaveBeenCalledWith("portal_preview_error_report", { id: "r1", body: expect.objectContaining({ message: "boom" }) }, "helsinki");
    for (let n = 0; n < MAX_ERRORS + 5; n += 1) await message(error(`e${n}`));
    expect(doorAction).toHaveBeenCalledTimes(MAX_ERRORS);
    doorAction.mockClear();
    await message(error("foreign"), window);
    expect(doorAction).not.toHaveBeenCalled();
  });

  it("counts a refused message and asks a ready frame of a live run to observe its version once", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const { unmount } = renderHook(() => usePreviewBridge(ref, run, true));
    await message({ kind: "nonsense" });
    expect(warn).toHaveBeenCalledWith("preview bridge: refused message 1 from the preview frame");
    await message({ kind: "jc-ready" });
    await message({ kind: "jc-ready" });
    expect(posted).toHaveBeenCalledTimes(1);
    expect(posted).toHaveBeenCalledWith({ kind: "jc-observe", version: 4 }, "*");
    unmount();
    await message({ kind: "nonsense" });
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it("serves a read under the run's other endpoint with the session", async () => {
    const fetchSpy = vi.fn(async () => new Response("[]", { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchSpy);
    renderHook(() => usePreviewBridge(ref, run, false));
    await message({ kind: "jc-request", id: 1, method: "GET", path: `/api/endpoint/${OTHER}/ngsi-ld/v1/entities?type=A` });
    await vi.waitFor(() => expect(posted).toHaveBeenCalledWith({ kind: "jc-response", id: 1, status: 200, body: [] }, "*"));
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });

  it("listens to nothing without a run, a project or a slug", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    renderHook(() => usePreviewBridge(ref, undefined, true));
    renderHook(() => usePreviewBridge(ref, { ...run, endpointSlug: undefined }, true));
    await message({ kind: "nonsense" });
    await message(error("boom"));
    expect(warn).not.toHaveBeenCalled();
    expect(doorAction).not.toHaveBeenCalled();
  });
});

describe("the defaults the page uses", () => {
  it("relays an observation to the run's report action, once per version", async () => {
    const relay = observationRelay("helsinki", "r1");
    const observation = { version: 2, pages: [], failedRequests: [] } as unknown as Parameters<typeof relay>[0];
    relay(observation);
    relay(observation);
    expect(doorAction).toHaveBeenCalledTimes(1);
    expect(doorAction).toHaveBeenCalledWith("portal_preview_observation_report", { id: "r1", body: observation }, "helsinki");
  });

  it("performs a request with the global fetch", async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchSpy);
    const source = { postMessage: vi.fn() } as unknown as Window;
    const outcome = await handleBridgeMessage(
      { source, data: { kind: "jc-request", id: 2, method: "GET", path: `/api/endpoint/${SLUG}/ngsi-ld/v1/entities` } },
      { slug: SLUG, operations: new Set(), source },
      () => undefined,
    );
    expect(outcome).toBe("forwarded");
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect((source.postMessage as ReturnType<typeof vi.fn>).mock.calls[0][0]).toEqual({ kind: "jc-response", id: 2, status: 204, body: null });
  });
});
