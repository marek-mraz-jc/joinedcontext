import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, Problem, server } from "./server";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the App's server", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("answers at /apps/{name}/api, and a name that is not an App's never builds a path", () => {
    expect(apiBase("news-topics")).toBe("/apps/news-topics/api");
    expect(apiBase(undefined)).toBe("/api");
    expect(apiBase("../x")).toBe("/api");
  });

  it("reads the weeks and a week's corpus URL, and an answer that is not a success is a Problem with its detail", async () => {
    const fetch = vi.fn(async (url: string, _init?: RequestInit) => {
      if (url === "/api/weeks") return json(200, { weeks: [], stale: false });
      if (url === "/api/weeks/2026-W41/corpus") return json(200, { url: "https://store.test/c.json" });
      if (url === "/api/weeks/2026-W40/corpus") return json(404, { title: "Not Found", detail: "the server keeps no such week" });
      return new Response("<html>", { status: 502 });
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect(await api.weeks()).toEqual({ weeks: [], stale: false });
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ credentials: "same-origin" });
    expect(await api.corpusUrl("2026-W41")).toBe("https://store.test/c.json");
    const refused = await api.corpusUrl("2026-W40").catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(Problem);
    expect(refused).toMatchObject({ status: 404, message: "the server keeps no such week" });
    await expect(api.corpusUrl("x/../y")).rejects.toMatchObject({ status: 502, message: "HTTP 502" });
    expect(fetch.mock.calls.at(-1)?.[0]).toBe("/api/weeks/x%2F..%2Fy/corpus");
  });

  it("asks who reads, follows and names topics and reads the history (T-3828)", async () => {
    const calls: Array<[string, string, unknown]> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const method = init?.method ?? "GET";
        const body = init?.body ? JSON.parse(String(init.body)) : undefined;
        calls.push([method, url, body]);
        if (method === "DELETE") return new Response(null, { status: 204 });
        if (url.endsWith("/me")) return json(200, null);
        if (method === "POST") return json(201, { id: 3, ownerName: "Sara", ...body });
        return json(200, []);
      }),
    );
    const api = server("/api");
    expect(await api.me()).toBeNull();
    expect(await api.follows()).toEqual([]);
    expect(await api.follow("kirjasto")).toEqual({ id: 3, ownerName: "Sara", term: "kirjasto" });
    expect(await api.unfollow(3)).toBeUndefined();
    expect(await api.names()).toEqual([]);
    expect(await api.name("kirjasto", "Libraries")).toMatchObject({ id: 3, term: "kirjasto", name: "Libraries" });
    expect(await api.unname(3)).toBeUndefined();
    expect(await api.changes()).toEqual([]);
    expect(calls).toEqual([
      ["GET", "/api/me", undefined],
      ["GET", "/api/follows", undefined],
      ["POST", "/api/follows", { term: "kirjasto" }],
      ["DELETE", "/api/follows/3", undefined],
      ["GET", "/api/names", undefined],
      ["POST", "/api/names", { term: "kirjasto", name: "Libraries" }],
      ["DELETE", "/api/names/3", undefined],
      ["GET", "/api/history", undefined],
    ]);
  });
});
