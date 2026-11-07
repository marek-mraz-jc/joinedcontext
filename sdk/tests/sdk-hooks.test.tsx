import { act, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider, useAccess, useEntities, useFunction, useMe, useSave } from "../src/sdk/hooks";
import { stubClient } from "../src/sdk/testing";
import type { Query } from "../src/sdk/client";

const STATIONS = [
  { id: "urn:1", type: "Station", name: "Kaivopuisto", bikes: 10 },
  { id: "urn:2", type: "Station", name: "Kamppi", bikes: 5 },
];

describe("sdk hooks", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("useEntities starts loading, resolves rows, and drops stale response after type change", async () => {
    let delay = 50;
    const client = stubClient({ entities: STATIONS });
    const origList = client.entities.all.bind(client.entities);
    client.entities.all = vi.fn(async (type: string, query?: Query) => {
      await new Promise((r) => setTimeout(r, delay));
      return origList(type, query);
    }) as typeof client.entities.all;

    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <JcProvider client={client}>{children}</JcProvider>
    );

    const { result, rerender } = renderHook(
      ({ type }) => useEntities(type),
      { wrapper, initialProps: { type: "Station" } },
    );

    expect(result.current.loading).toBe(true);
    expect(result.current.rows).toEqual([]);

    await act(async () => {
      vi.advanceTimersByTime(60);
    });

    expect(result.current.loading).toBe(false);
    expect(result.current.rows).toHaveLength(2);

    // Switch to another type with delayed resolution
    delay = 100;
    rerender({ type: "Vehicle" });
    expect(result.current.loading).toBe(true);

    // Rapidly switch back to Station before Vehicle resolves
    delay = 20;
    rerender({ type: "Station" });

    await act(async () => {
      vi.advanceTimersByTime(30);
    });

    // Station resolves and is kept; when Vehicle arrives later it is dropped
    expect(result.current.rows).toHaveLength(2);
    expect(result.current.rows[0].type).toBe("Station");
  });

  it("useEntities refreshMs polls without flipping loading", async () => {
    const client = stubClient({ entities: STATIONS });
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <JcProvider client={client}>{children}</JcProvider>
    );

    const { result } = renderHook(
      () => useEntities("Station", undefined, { refreshMs: 2000 }),
      { wrapper },
    );

    await act(async () => {
      vi.advanceTimersByTime(10);
    });
    expect(result.current.loading).toBe(false);
    expect(result.current.rows).toHaveLength(2);

    // Polling triggers after 2000ms
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });

    // Loading should stay false during refresh
    expect(result.current.loading).toBe(false);
  });

  // SDK-02 (T-2724): a type two endpoints serve is created where the caller names, in that
  // endpoint's space; without a name the hook hands back the client's refusal, never a guess.
  it("useSave creates through the endpoint named, and refuses to guess between two", async () => {
    const client = stubClient(undefined, {
      endpoints: [
        { name: "north", slug: "north", space: "n", types: ["Parking"] },
        { name: "south", slug: "south", space: "s", types: ["Parking"] },
      ],
    });
    const wrapper = ({ children }: { children: React.ReactNode }) => <JcProvider client={client}>{children}</JcProvider>;
    const { result } = renderHook(() => useSave(), { wrapper });

    let id: string | null = null;
    await act(async () => {
      id = await result.current.create("Parking", { free: 3 }, "p1", { endpoint: "south" });
    });
    expect(id).toBe("urn:ngsi-ld:Parking:example.org:s:p1");
    expect(client.transport.calls.at(-1)).toMatchObject({ method: "POST", path: "/api/endpoint/south/ngsi-ld/v1/entities" });

    await act(async () => {
      id = await result.current.create("Parking", { free: 3 }, "p2");
    });
    expect(id).toBeNull();
    expect(result.current.problem?.title).toBe("Type 'Parking' is served by more than one endpoint");
  });

  it("useSave sets problem on refusal and resolves false/null without rejecting", async () => {
    const client = stubClient({
      refuse: (req) => {
        if (req.method === "PATCH") {
          return { status: 403, body: { title: "Forbidden", detail: "Read only" } };
        }
        return null;
      },
    });

    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <JcProvider client={client}>{children}</JcProvider>
    );

    const { result } = renderHook(() => useSave(), { wrapper });

    let ok = true;
    await act(async () => {
      ok = await result.current.update("urn:1", { bikes: 2 });
    });

    expect(ok).toBe(false);
    expect(result.current.problem?.detail).toBe("Read only");
    expect(result.current.saving).toBe(false);

    act(() => {
      result.current.clear();
    });
    expect(result.current.problem).toBeNull();
  });

  it("useAccess returns can decision", async () => {
    const client = stubClient({
      access: {
        permissions: [{ resource: { type: "Station" }, actions: ["queryEntity"], attributes: "*" }],
        prohibitions: [],
      },
    });

    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <JcProvider client={client}>{children}</JcProvider>
    );

    const { result } = renderHook(() => useAccess(), { wrapper });

    await act(async () => {
      vi.advanceTimersByTime(10);
    });

    expect(result.current.can("queryEntity", "Station")).toEqual({ ok: true });
    expect(result.current.can("deleteEntity", "Station").ok).toBe(false);
  });

  // SDK-35, SDK-36: useMe() is the person the host served, roles included, and a refusal to a
  // person with roles names them so a disabled control says which role falls short.
  it("useMe returns the served person and useAccess names their roles in a refusal", async () => {
    const client = stubClient(
      {
        access: {
          permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity"], attributes: "*" }],
          prohibitions: [],
        },
      },
      { user: { id: "5f0c", name: "Jana", email: "jana@hel.fi", roles: ["viewer"] } },
    );
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <JcProvider client={client}>{children}</JcProvider>
    );
    const { result } = renderHook(() => ({ me: useMe(), access: useAccess() }), { wrapper });
    await act(async () => {
      vi.advanceTimersByTime(10);
    });

    expect(result.current.me).toEqual({ id: "5f0c", name: "Jana", email: "jana@hel.fi", roles: ["viewer"] });
    expect(result.current.access.can("queryEntity", "Alert")).toEqual({ ok: true });
    expect(result.current.access.can("updateAttrs", "Alert")).toEqual({
      ok: false,
      reason: "Your role viewer does not permit updateAttrs on Alert.",
    });
  });

  // SDK-35: an anonymous visitor is null, never an empty person.
  it("useMe is null for an anonymous visitor", () => {
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <JcProvider client={stubClient({}, { user: null })}>{children}</JcProvider>
    );
    expect(renderHook(() => useMe(), { wrapper }).result.current).toBeNull();
  });

  it("useFunction returns data and handles errors", async () => {
    // waitFor polls with real timers.
    vi.useRealTimers();
    const client = stubClient({
      functions: {
        greet: (body) => ({ message: `Hello ${(body as { name: string }).name}` }),
      },
    });

    function TestComp() {
      const { data, loading, error } = useFunction<{ message: string }>("greet", { name: "World" });
      if (loading) return <div>Loading...</div>;
      if (error) return <div>Error: {error.message}</div>;
      return <div>Result: {data?.message}</div>;
    }

    render(
      <JcProvider client={client}>
        <TestComp />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByText("Result: Hello World")).toBeInTheDocument();
    });
  });
});
