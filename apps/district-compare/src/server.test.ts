import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, Problem, server } from "./server";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the App's server", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("answers at /apps/{name}/api, and a name that is not an App's never builds a path", () => {
    expect(apiBase("district-compare")).toBe("/apps/district-compare/api");
    expect(apiBase(undefined)).toBe("/api");
    expect(apiBase("../x")).toBe("/api");
  });

  it("asks for the history of the codes and the files, and an answer that is not a success is a Problem with its detail", async () => {
    const fetch = vi.fn(async (url: string, _init?: RequestInit) => {
      if (url === "/api/metrics?codes=101%2C102") return json(200, { days: [], stale: false });
      if (url === "/api/boundaries") return json(404, { title: "Not Found", detail: "the server has not kept the boundaries yet" });
      return new Response("<html>", { status: 502 });
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect(await api.history(["101", "102"])).toEqual({ days: [], stale: false });
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ credentials: "same-origin" });
    const refused = await api.files().catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(Problem);
    expect(refused).toMatchObject({ status: 404, message: "the server has not kept the boundaries yet" });
    await expect(api.history(["x"])).rejects.toMatchObject({ status: 502, message: "HTTP 502" });
  });

  it("keeps, opens, changes and deletes comparisons, pins one, and reads who is signed in and the changes", async () => {
    const c = { name: "Centre", codes: ["101"], measure: "events" as const };
    const fetch = vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url === "/api/me") return json(200, null);
      if (url === "/api/comparisons" && method === "GET") return json(200, []);
      if (url === "/api/comparisons" && method === "POST") return json(201, { ...JSON.parse(String(init?.body)), id: 3, ownerName: "V" });
      if (url === `/api/shared/${"ab".repeat(32)}`) return json(200, { ...c, id: 3, ownerName: "V", mine: true });
      if (url === "/api/comparisons/3") return new Response(null, { status: 204 });
      if (url === "/api/default" && method === "GET") return json(200, null);
      if (url === "/api/default" && method === "PUT") return json(200, { ...c, id: 1, ownerName: "S" });
      if (url === "/api/default") return json(403, { detail: "this needs the role steward" });
      if (url === "/api/history") return json(200, []);
      return new Response("<html>", { status: 502 });
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect(await api.me()).toBeNull();
    expect(await api.comparisons()).toEqual([]);
    expect(await api.keep(c)).toMatchObject({ ...c, id: 3 });
    expect(fetch.mock.calls[2]?.[1]).toMatchObject({ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(c) });
    expect(await api.shared("ab".repeat(32))).toMatchObject({ mine: true });
    expect(await api.change(3, c)).toBeUndefined();
    expect(await api.forget(3)).toBeUndefined();
    expect(await api.pinned()).toBeNull();
    expect(await api.pin(c)).toMatchObject({ id: 1 });
    await expect(api.unpin()).rejects.toMatchObject({ status: 403, message: "this needs the role steward" });
    expect(await api.changes()).toEqual([]);
  });
});
