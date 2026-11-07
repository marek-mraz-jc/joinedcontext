/**
 * The run stream hook: every kind the Portal writes arrives (F-UI2-010), and a switch to another
 * run shows that run's events alone (F-UI2-011).
 */
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAgentRun } from "../src/pages/apps/useAgentRun";

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
