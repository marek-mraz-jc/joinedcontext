// T-7706: TypeQuality read and retried from the API, the Developer panel's try-it when the network fails, and the
// agent run hook's reset.
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { TypeQuality } from "../src/pages/explore/TypeQuality";
import { TryIt } from "../src/pages/endpoints/DeveloperPanel";
import { useAgentRun } from "../src/pages/apps/useAgentRun";
import { InRouter } from "./pageHarness";
import { doorsAsRest } from "./odata";

const TYPE = "BikeHireDockingStation";
const REPORT = {
  observedAt: "2026-10-07T02:00:00Z",
  checked: 10,
  invalid: 0,
  truncated: false,
  rules: [],
  freshness: [],
  types: [{ type: TYPE, count: 10, newest: "2026-10-07T01:58:00Z", attributes: [{ name: "name", present: 7, outliers: 0, outlierExamples: [] }] }],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function app(node: ReactNode) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>{node}</InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("TypeQuality reads the report from the API", () => {
  it("shows the failure, and the report once the person retries", async () => {
    let reads = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url);
        if (!url.pathname.endsWith("/quality")) return new Response("{}", { headers: { "Content-Type": "application/json" } });
        reads += 1;
        return reads === 1
          ? new Response(JSON.stringify({ title: "x", detail: "the quality run is unavailable", status: 503 }), { status: 503, headers: { "Content-Type": "application/problem+json" } })
          : new Response(JSON.stringify(REPORT), { headers: { "Content-Type": "application/json" } });
      }),
    );
    doorsAsRest();
    app(<TypeQuality project="helsinki" space="helsinki" type={TYPE} />);
    expect(await screen.findByText(/the quality run is unavailable/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByTestId("type-quality")).toBeInTheDocument();
    expect(reads).toBe(2);
  });
});

describe("TryIt when the network fails", () => {
  it("says why nothing came back and keeps the button usable", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new TypeError("connection refused"))));
    app(<TryIt slug="air" path="/ngsi-ld/v1/entities?type=Air" accept="application/json" editable />);
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.developer.send }));
    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.getByRole("button", { name: en.endpoints.developer.send })).toBeEnabled();
  });

  it("will not send a typed path that leaves the endpoint", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    app(<TryIt slug="air" path="/ngsi-ld/v1/entities" accept="application/json" editable />);
    const box = await screen.findByRole("textbox");
    fireEvent.change(box, { target: { value: "/a/../b" } });
    const send = screen.getByRole("button", { name: en.endpoints.developer.send });
    expect(send).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(send);
    expect(screen.getAllByText(en.endpoints.developer.problem.outside).length).toBeGreaterThan(0);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("useAgentRun reset", () => {
  it("drops the events of the run it holds", async () => {
    class Stub {
      private listeners = new Map<string, Set<EventListener>>();
      addEventListener(kind: string, listener: EventListener) {
        this.listeners.set(kind, (this.listeners.get(kind) ?? new Set()).add(listener));
      }
      removeEventListener() {}
      close() {}
      emit(kind: string, seq: number) {
        for (const listener of this.listeners.get(kind) ?? []) listener(new MessageEvent(kind, { data: JSON.stringify({ seq }), lastEventId: String(seq) }));
      }
    }
    const opened: Stub[] = [];
    vi.stubGlobal("EventSource", function (this: unknown) {
      const one = new Stub();
      opened.push(one);
      return one;
    } as unknown as typeof EventSource);
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response("{}", { headers: { "Content-Type": "application/json" } }))));
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useAgentRun("helsinki", "run-a"), { wrapper });
    await waitFor(() => expect(opened.length).toBeGreaterThan(0));
    act(() => opened[opened.length - 1].emit("app-build", 3));
    expect(result.current.events).toHaveLength(1);
    act(() => result.current.reset());
    expect(result.current.events).toHaveLength(0);
  });
});
