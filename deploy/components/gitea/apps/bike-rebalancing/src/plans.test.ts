import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, plansApi, ServerProblem } from "./plans";

function answer(status: number, body: unknown): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

// T-3346: the page talks to its own server, below /apps/{name}/api, as the signed-in caller.
describe("plansApi", () => {
  it("names the App's own server from #jc-config, and never a name that could leave its path", () => {
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"bike-rebalancing"}</script>`;
    expect(apiBase()).toBe("/apps/bike-rebalancing/api");
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"../portal"}</script>`;
    expect(apiBase()).toBe("/apps/bike-rebalancing/api");
    document.body.innerHTML = `<script id="jc-config" type="application/json">not json</script>`;
    expect(apiBase()).toBe("/apps/bike-rebalancing/api");
  });

  it("lists one operator's plans with the name encoded, and all when none is named", async () => {
    const fetch = vi.fn(async (_url: string, _init?: RequestInit) => answer(200, []));
    vi.stubGlobal("fetch", fetch);
    const api = plansApi("/apps/x/api");
    await api.list(" Van 2&co ");
    await api.list("  ");
    expect(fetch.mock.calls[0][0]).toBe("/apps/x/api/plans?operator=Van%202%26co");
    expect(fetch.mock.calls[1][0]).toBe("/apps/x/api/plans");
    expect(fetch.mock.calls[0][1]).toMatchObject({ method: "GET", credentials: "same-origin" });
  });

  it("sends a plan's choices as JSON and a drive's stops", async () => {
    const fetch = vi.fn().mockResolvedValueOnce(answer(201, { id: 4 })).mockResolvedValueOnce(answer(201, { id: 1 }));
    vi.stubGlobal("fetch", fetch);
    const api = plansApi("/a");
    await api.save({ operator: "Van 1", vanCapacity: 12, start: null, include: ["s1"], exclude: [] });
    await api.drive(4, ["s1", "s2"]);
    expect(JSON.parse(fetch.mock.calls[0][1].body as string)).toEqual({ operator: "Van 1", vanCapacity: 12, start: null, include: ["s1"], exclude: [] });
    expect(fetch.mock.calls[1][0]).toBe("/a/plans/4/drives");
    expect(JSON.parse(fetch.mock.calls[1][1].body as string)).toEqual({ stops: ["s1", "s2"] });
  });

  it("turns a refusal into its status and words, and a dead network into status 0", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(answer(400, { detail: "name the van" })).mockRejectedValueOnce(new TypeError("offline")).mockResolvedValueOnce(new Response("<html>", { status: 502 })));
    const api = plansApi("/a");
    await expect(api.save({ operator: "", vanCapacity: 1, start: null, include: [], exclude: [] })).rejects.toMatchObject({ status: 400, message: "name the van" });
    await expect(api.list("")).rejects.toMatchObject({ status: 0 });
    const error = await api.remove(1).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ServerProblem);
    expect(error).toMatchObject({ status: 502, message: "" });
  });

  it("reads one plan and the route sheet's URL", async () => {
    const fetch = vi
      .fn(async (_url: string, _init?: RequestInit) => answer(200, {}))
      .mockResolvedValueOnce(answer(200, { id: 7, operator: "Van 1" }))
      .mockResolvedValueOnce(answer(200, { url: "https://store.example/s.csv" }));
    vi.stubGlobal("fetch", fetch);
    const api = plansApi("/a");
    await expect(api.get(7)).resolves.toMatchObject({ id: 7 });
    await expect(api.sheetUrl(7)).resolves.toBe("https://store.example/s.csv");
    expect(fetch.mock.calls.map((call) => call[0])).toEqual(["/a/plans/7", "/a/plans/7/sheet"]);
  });

  it("reads a deletion's empty answer as done", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 204 })));
    await expect(plansApi("/a").remove(3)).resolves.toBeUndefined();
  });
});
