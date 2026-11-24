/**
 * What the space data page keeps beside its grid (T-3107, T-6210): a delete keeps the person's
 * copy in the trash first, and the trash puts a row back with the person's own create.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/DataViews.tsx.
import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { sentTo } from "./requests";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SourceError, toRichRow } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import { deleteRow, LiveNotice, TrashPanel, useLive, writable } from "../src/pages/spaces/DataViews";

const station = (n: number, extra: Record<string, unknown> = {}) =>
  toRichRow(
    {
      id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:${n}`,
      type: "BikeHireDockingStation",
      name: { type: "Property", value: `Station ${n}` },
      availableBikeNumber: { type: "Property", value: n },
      totalSlotNumber: { type: "Property", value: 20 },
      address: { type: "Property", value: `Street ${n}` },
      status: { type: "Property", value: "working" },
      operator: { type: "Property", value: "HSL" },
      zone: { type: "Property", value: "A" },
      ...extra,
    },
    "en",
  );


describe("delete and the trash (T-3107)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const json = (body: unknown, status = 200) =>
    Promise.resolve(
      status === 204
        ? new Response(null, { status })
        : new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
    );

  it("sends the entity back without the broker's own timestamps", () => {
    expect(
      writable({
        id: "urn:ngsi-ld:T:1",
        type: "T",
        createdAt: "x",
        modifiedAt: "y",
        a: { type: "Property", value: 1, createdAt: "x", observedAt: "z" },
      }),
    ).toEqual({ id: "urn:ngsi-ld:T:1", type: "T", a: { type: "Property", value: 1, observedAt: "z" } });
  });

  it("strips the timestamps inside each instance of a multi-instance attribute", () => {
    expect(writable({ id: "u", a: [{ value: 1, modifiedAt: "x" }, { value: 2, deletedAt: "y" }], tags: ["x", null] })).toEqual({
      id: "u",
      a: [{ value: 1 }, { value: 2 }],
      tags: ["x", null],
    });
  });

  it("refuses a delete on a view without a delete, before anything is sent", async () => {
    const fetchMock = vi.fn(() => json(null, 204));
    vi.stubGlobal("fetch", fetchMock);
    await expect(deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn() }, station(1), "a duplicate")).rejects.toThrow("this view cannot delete");
    expect(fetchMock).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("keeps the copy before the delete, and forgets it when the gateway refuses the delete", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      return request.method === "POST" ? json({ id: 9, urn: "u", type: "T", entity: {}, deletedAt: "", expiresAt: "" }, 201) : json(null, 204);
    });
    vi.stubGlobal("fetch", fetchMock);
    const remove = vi.fn(() => Promise.resolve());
    await deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove }, station(1), "a duplicate");
    const kept = sentTo(fetchMock, "/spaces/bikes/trash");
    expect(kept.map((call) => call.method)).toEqual(["POST"]);
    expect(((await kept[0].json()) as { entity: { id: string } }).entity.id).toBe(station(1).id);
    expect(remove).toHaveBeenCalledWith(station(1).id, "a duplicate");

    const refused = vi.fn(() => Promise.reject(new SourceError(403, "no delete granted")));
    await expect(deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove: refused }, station(2), "a duplicate")).rejects.toThrow(
      "no delete granted",
    );
    expect(sentTo(fetchMock, "/spaces/bikes/trash/9").map((call) => call.method)).toEqual(["DELETE"]);
    vi.unstubAllGlobals();
  });

  // DM-113: no reason, no copy and no delete: the gateway would refuse it, so nothing is sent.
  it("refuses a delete without a reason before it keeps a copy", async () => {
    const fetchMock = vi.fn(() => json(null, 204));
    vi.stubGlobal("fetch", fetchMock);
    const remove = vi.fn(() => Promise.resolve());
    for (const bad of ["", "  ", "x".repeat(501)]) {
      await expect(deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove }, station(1), bad)).rejects.toThrow("1 to 500");
    }
    expect(fetchMock).not.toHaveBeenCalled();
    expect(remove).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("says so when the copy of a refused delete stays in the trash", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      return request.method === "POST" ? json({ id: 9, urn: "u", type: "T", entity: {}, deletedAt: "", expiresAt: "" }, 201) : json({ title: "down" }, 500);
    });
    vi.stubGlobal("fetch", fetchMock);
    const refused = vi.fn(() => Promise.reject(new SourceError(403, "no delete granted")));
    await expect(deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove: refused }, station(2), "a duplicate")).rejects.toThrow(
      "no delete granted. Its copy also stays among your deleted entities.",
    );
    vi.unstubAllGlobals();
  });

  it("lists the person's deleted entities and puts one back with their own create", async () => {
    const item = { id: 4, urn: station(1).id, type: "BikeHireDockingStation", entity: { id: station(1).id, type: "BikeHireDockingStation", name: { type: "Property", value: "Station 1" } }, deletedAt: "2026-10-06T19:20:00Z", expiresAt: "2026-11-05T19:20:00Z" };
    let listed = [item];
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      if (request.method === "DELETE") {
        listed = [];
        return json(null, 204);
      }
      return json(listed);
    });
    vi.stubGlobal("fetch", fetchMock);
    const restore = vi.fn(() => Promise.resolve());
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <TrashPanel project="helsinki" space="bikes" restore={restore} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const panel = await screen.findByTestId("view-trash");
    expect(within(panel).getByText("Deleted entities (1)")).toBeInTheDocument();
    await userEvent.click(within(panel).getByText("Deleted entities (1)"));
    await userEvent.click(within(panel).getByRole("button", { name: "Put back Station 1" }));
    expect(restore).toHaveBeenCalledWith(item.entity);
    await waitFor(() => expect(screen.queryByTestId("view-trash")).toBeNull());
    expect(sentTo(fetchMock, "/spaces/bikes/trash/4").map((call) => call.method)).toEqual(["DELETE"]);
    vi.unstubAllGlobals();
  });

  it("says a put-back entity's copy stayed when the trash keeps it", async () => {
    const item = { id: 4, urn: station(1).id, type: "BikeHireDockingStation", entity: { id: station(1).id, type: "BikeHireDockingStation", name: { type: "Property", value: "Station 1" } }, deletedAt: "2026-10-06T19:20:00Z", expiresAt: "2026-11-05T19:20:00Z" };
    vi.stubGlobal(
      "fetch",
      // The copy's forget reaches its REST door through the Portal door; its refusal is said in the door's words.
      vi.fn((input: RequestInfo | URL) => ((input as Request).method === "DELETE" ? json({ title: "down" }, 500) : json([item]))),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <TrashPanel project="helsinki" space="bikes" restore={vi.fn(() => Promise.resolve())} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const panel = await screen.findByTestId("view-trash");
    await userEvent.click(within(panel).getByText("Deleted entities (1)"));
    await userEvent.click(within(panel).getByRole("button", { name: "Put back Station 1" }));
    expect(await within(panel).findByText(/Station 1 is back, but its copy stays among your deleted entities \(down\)/)).toBeInTheDocument();
    vi.unstubAllGlobals();
  });

  it("says the gateway's words when it refuses a put-back, and keeps the row in the trash", async () => {
    const item = { id: 4, urn: station(1).id, type: "BikeHireDockingStation", entity: { id: station(1).id, type: "BikeHireDockingStation" }, deletedAt: "2026-10-06T19:20:00Z", expiresAt: "2026-11-05T19:20:00Z" };
    const fetchMock = vi.fn(() => json([item]));
    vi.stubGlobal("fetch", fetchMock);
    const restore = vi
      .fn<(entity: Record<string, unknown>) => Promise<void>>()
      .mockRejectedValueOnce(new SourceError(409, "the URN exists again"))
      .mockRejectedValueOnce("offline");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <TrashPanel project="helsinki" space="bikes" restore={restore} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const panel = await screen.findByTestId("view-trash");
    await userEvent.click(within(panel).getByText("Deleted entities (1)"));
    // No name: the URN stands for the entity.
    const button = within(panel).getByRole("button", { name: `Put back ${station(1).id}` });
    await userEvent.click(button);
    expect((await within(panel).findByRole("alert")).textContent).toContain("the URN exists again");
    await userEvent.click(button);
    expect((await within(panel).findByRole("alert")).textContent).toContain("offline");
    expect(sentTo(fetchMock, "/spaces/bikes/trash/4")).toEqual([]);
    expect(screen.getByTestId("view-trash")).toBeInTheDocument();
    vi.unstubAllGlobals();
  });

  it("shows no trash when the answer is no list", async () => {
    vi.stubGlobal("fetch", vi.fn(() => json({ unexpected: true })));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <TrashPanel project="helsinki" space="bikes" restore={vi.fn()} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(client.getQueryData(["projects", "helsinki", "spaces", "bikes", "trash"])).toEqual([]));
    expect(screen.queryByTestId("view-trash")).toBeNull();
    vi.unstubAllGlobals();
  });
});

describe("the live notice (T-3105)", () => {
  class FakeSource {
    static last: FakeSource | null = null;
    url: string;
    closed = false;
    listeners = new Map<string, EventListener>();
    constructor(url: string) {
      this.url = url;
      FakeSource.last = this;
    }
    addEventListener(name: string, listener: EventListener) {
      this.listeners.set(name, listener);
    }
    removeEventListener(name: string) {
      this.listeners.delete(name);
    }
    close() {
      this.closed = true;
    }
    emit(data: string) {
      this.listeners.get("changed")?.({ data } as unknown as Event);
    }
  }

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    FakeSource.last = null;
  });

  it("listens on the type's live stream and reports each change event of a type", () => {
    vi.stubGlobal("EventSource", FakeSource);
    const onChange = vi.fn();
    const { unmount } = renderHook(() => useLive("hel sinki", "bikes/1", "Bike Station", onChange));
    const source = FakeSource.last!;
    expect(source.url).toBe("/api/v1/projects/hel%20sinki/spaces/bikes%2F1/live?type=Bike%20Station");
    source.emit(JSON.stringify({ type: "BikeHireDockingStation" }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0].type).toBe("BikeHireDockingStation");
    expect(onChange.mock.calls[0][0].at).toBeInstanceOf(Date);
    // Not JSON, or JSON naming no type: nothing to act on.
    source.emit("not json");
    source.emit(JSON.stringify({ type: 3 }));
    expect(onChange).toHaveBeenCalledTimes(1);
    unmount();
    expect(source.closed).toBe(true);
    expect(source.listeners.size).toBe(0);
    vi.unstubAllGlobals();
  });

  it("calls the newest callback, and opens nothing without a type or an EventSource", () => {
    vi.stubGlobal("EventSource", FakeSource);
    const first = vi.fn();
    const second = vi.fn();
    const { rerender, unmount } = renderHook(({ cb }) => useLive("p", "s", "T", cb), { initialProps: { cb: first } });
    rerender({ cb: second });
    act(() => FakeSource.last!.emit(JSON.stringify({ type: "T" })));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
    unmount();
    FakeSource.last = null;
    renderHook(() => useLive("p", "s", "", vi.fn()));
    expect(FakeSource.last).toBeNull();
    vi.stubGlobal("EventSource", undefined);
    renderHook(() => useLive("p", "s", "T", vi.fn()));
    expect(FakeSource.last).toBeNull();
    vi.unstubAllGlobals();
  });

  it("says when others changed rows, and nothing before", () => {
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <LiveNotice change={null} />
      </I18nextProvider>,
    );
    expect(screen.getByTestId("view-live").textContent).toBe("");
    const at = new Date("2026-11-23T10:15:00Z");
    rerender(
      <I18nextProvider i18n={i18n}>
        <LiveNotice change={{ type: "T", at }} />
      </I18nextProvider>,
    );
    expect(screen.getByTestId("view-live").textContent).toContain(at.toLocaleTimeString("en"));
    expect(screen.getByRole("status")).toHaveAttribute("aria-live", "polite");
  });
});

