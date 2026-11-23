/**
 * The run stream hook: every kind the Portal writes arrives (F-UI2-010), and a switch to another
 * run shows that run's events alone (F-UI2-011).
 */
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runInUrl, setRunInUrl, useAgentRun } from "../src/pages/apps/useAgentRun";

class StubEventSource {
  static opened: StubEventSource[] = [];
  readonly url: string;
  private readonly listeners = new Map<string, Set<EventListener>>();
  constructor(url: string) {
    this.url = url;
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
  close(): void {}
  emit(kind: string, payload: { seq: number }): void {
    const frame = new MessageEvent(kind, { data: JSON.stringify(payload), lastEventId: String(payload.seq) });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) listener(frame);
  }
}

const source = (runId: string) => StubEventSource.opened.filter((one) => one.url.includes(`/${runId}/events`)).at(-1) as StubEventSource;

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("useAgentRun", () => {
  beforeEach(() => {
    StubEventSource.opened = [];
    vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }))));
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("hears the app-build card Build an app answers with", async () => {
    const { result } = renderHook(() => useAgentRun("helsinki", "run-a"), { wrapper });
    await waitFor(() => expect(source("run-a")).toBeDefined());
    act(() => source("run-a").emit("app-build", { seq: 4 }));
    expect(result.current.events.map((event) => event.kind)).toEqual(["app-build"]);
  });

  it("is streaming from the stream's open until its error, and again once it reopens", async () => {
    const { result } = renderHook(() => useAgentRun("helsinki", "run-a"), { wrapper });
    await waitFor(() => expect(source("run-a")).toBeDefined());
    expect(result.current.streaming).toBe(false);
    act(() => source("run-a").emit("open", { seq: 0 }));
    expect(result.current.streaming).toBe(true);
    act(() => source("run-a").emit("error", { seq: 0 }));
    expect(result.current.streaming).toBe(false);
    act(() => source("run-a").emit("open", { seq: 0 }));
    expect(result.current.streaming).toBe(true);
  });

  it("publishes the run through its door and hands back the Change it opened", async () => {
    const change = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-0000a1b2" } };
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      const answer = request.url.endsWith("/publish") ? change : {};
      return Promise.resolve(new Response(JSON.stringify(answer), { status: request.url.endsWith("/publish") ? 202 : 200, headers: { "Content-Type": "application/json" } }));
    });
    vi.stubGlobal("fetch", fetchMock);
    const { result } = renderHook(() => useAgentRun("helsinki", "run-a"), { wrapper });
    await waitFor(() => expect(source("run-a")).toBeDefined());

    let published: unknown;
    await act(async () => {
      published = await result.current.publish.mutateAsync();
    });

    expect(published).toEqual(change);
    const asked = fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.url.endsWith("/publish"));
    expect(asked).toHaveLength(1);
    expect(asked[0].method).toBe("POST");
    expect(asked[0].url).toContain("run-a");
  });

  it("shows the next run's events alone after a switch", async () => {
    const { result, rerender } = renderHook(({ runId }) => useAgentRun("helsinki", runId), {
      wrapper,
      initialProps: { runId: "run-a" },
    });
    await waitFor(() => expect(source("run-a")).toBeDefined());
    act(() => [1, 2, 3].forEach((seq) => source("run-a").emit("thought", { seq })));
    rerender({ runId: "run-b" });
    await waitFor(() => expect(source("run-b")).toBeDefined());
    act(() => [1, 2, 3].forEach((seq) => source("run-b").emit("message", { seq })));
    expect(result.current.events.map((event) => `${event.kind}:${event.seq}`)).toEqual(["message:1", "message:2", "message:3"]);
  });
});

describe("the run named in the address", () => {
  afterEach(() => {
    window.history.replaceState(null, "", "/");
  });

  it("keeps the run in ?run= beside what else the address says, and takes it out again", () => {
    window.history.replaceState(null, "", "/projects/helsinki/apps?tab=builds");
    expect(runInUrl()).toBeNull();
    setRunInUrl("run-a");
    expect(runInUrl()).toBe("run-a");
    expect(window.location.pathname).toBe("/projects/helsinki/apps");
    expect(new URLSearchParams(window.location.search).get("tab")).toBe("builds");
    setRunInUrl("run-b");
    expect(runInUrl()).toBe("run-b");
    setRunInUrl(null);
    expect(runInUrl()).toBeNull();
    expect(window.location.search).toBe("?tab=builds");
  });
});
