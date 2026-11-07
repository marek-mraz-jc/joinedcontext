/**
 * What the space data page keeps beside its grid (T-3107, T-6210): a delete keeps the person's
 * copy in the trash first, and the trash puts a row back with the person's own create.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/DataViews.tsx.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { sentTo } from "./requests";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SourceError, toRichRow } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import { deleteRow, TrashPanel, writable } from "../src/pages/spaces/DataViews";

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

  it("keeps the copy before the delete, and forgets it when the gateway refuses the delete", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      return request.method === "POST" ? json({ id: 9, urn: "u", type: "T", entity: {}, deletedAt: "", expiresAt: "" }, 201) : json(null, 204);
    });
    vi.stubGlobal("fetch", fetchMock);
    const remove = vi.fn(() => Promise.resolve());
    await deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove }, station(1));
    const kept = sentTo(fetchMock, "/spaces/bikes/trash");
    expect(kept.map((call) => call.method)).toEqual(["POST"]);
    expect(((await kept[0].json()) as { entity: { id: string } }).entity.id).toBe(station(1).id);
    expect(remove).toHaveBeenCalledWith(station(1).id);

    const refused = vi.fn(() => Promise.reject(new SourceError(403, "no delete granted")));
    await expect(deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove: refused }, station(2))).rejects.toThrow(
      "no delete granted",
    );
    expect(sentTo(fetchMock, "/spaces/bikes/trash/9").map((call) => call.method)).toEqual(["DELETE"]);
    vi.unstubAllGlobals();
  });

  it("says so when the copy of a refused delete stays in the trash", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      return request.method === "POST" ? json({ id: 9, urn: "u", type: "T", entity: {}, deletedAt: "", expiresAt: "" }, 201) : json({ title: "down" }, 500);
    });
    vi.stubGlobal("fetch", fetchMock);
    const refused = vi.fn(() => Promise.reject(new SourceError(403, "no delete granted")));
    await expect(deleteRow("helsinki", "bikes", { query: vi.fn(), get: vi.fn(), remove: refused }, station(2))).rejects.toThrow(
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
    expect(await within(panel).findByText(/Station 1 is back, but its copy stays among your deleted entities \(HTTP 500\)/)).toBeInTheDocument();
    vi.unstubAllGlobals();
  });
});

