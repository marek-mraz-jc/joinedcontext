import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, Problem, server } from "./server";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the App's server", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("answers at /apps/{name}/api, and a name that is not an App's never builds a path", () => {
    expect(apiBase("data-quality-inspector")).toBe("/apps/data-quality-inspector/api");
    expect(apiBase(undefined)).toBe("/api");
    expect(apiBase("../x")).toBe("/api");
  });

  it("reads the runs, reads a report's URL, and an answer that is not a success is a Problem with its detail", async () => {
    const fetch = vi.fn(async (url: string, _init?: RequestInit) => {
      if (url === "/api/runs") return json(200, { runs: [], stale: false });
      if (url === "/api/runs/4/report") return json(200, { url: "https://store.test/r.json" });
      if (url === "/api/runs/5/report") return json(404, { title: "Not Found", detail: "no such run" });
      return new Response("<html>", { status: 502 });
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect(await api.runs()).toEqual({ runs: [], stale: false });
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ method: "GET", credentials: "same-origin" });
    expect(await api.reportUrl(4)).toBe("https://store.test/r.json");
    const refused = await api.reportUrl(5).catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(Problem);
    expect(refused).toMatchObject({ status: 404, message: "no such run" });
    await expect(api.reportUrl(6)).rejects.toMatchObject({ status: 502, message: "HTTP 502" });
  });

  // T-3827: who calls, the resolutions, a correction and the history, each at its route.
  it("calls each route of the roles with its method and body, reads a 204 as nothing, and a correction of a resolved finding as no new resolution", async () => {
    const fetch = vi.fn(async (url: string, init?: RequestInit) => {
      if (init?.method === "DELETE") return new Response(null, { status: 204 });
      if (url === "/api/corrections") return json(200, { entity: "e", attribute: "a", corrected: true });
      return json(200, url.endsWith("/resolutions") && init?.method === "POST" ? { id: 3 } : []);
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    await api.me();
    await api.resolutions();
    await api.resolve("e", "a", "n");
    await expect(api.reopen(3)).resolves.toBeUndefined();
    await expect(api.correct("e", "a", 4, "n")).resolves.toBeNull();
    await api.history();
    expect(fetch.mock.calls.map(([url, init]) => `${init?.method} ${url}`)).toEqual([
      "GET /api/me",
      "GET /api/resolutions",
      "POST /api/resolutions",
      "DELETE /api/resolutions/3",
      "POST /api/corrections",
      "GET /api/history",
    ]);
    expect(JSON.parse(fetch.mock.calls[4][1]?.body as string)).toEqual({ entity: "e", attribute: "a", value: 4, note: "n" });
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(api.me()).rejects.toMatchObject({ status: 0 });
  });
});
