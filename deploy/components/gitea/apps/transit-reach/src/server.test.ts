import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, keptReach, reachApi, ServerProblem } from "./server";
import type { KeptReach, ReachApi } from "./server";

function answer(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

const KEPT: KeptReach = { version: "v1", stop: "urn:ngsi-ld:GtfsStop:HSL:1", bands: [{ minutes: 10, areaKm2: 1.5, stops: 3 }], cached: false, stale: false, url: "https://store.example/t.geojson" };

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

// T-3349: the areas from a stop, kept on the App's own server.
describe("reachApi", () => {
  it("names the App's own server, never a name that leaves its path", () => {
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"transit-reach"}</script>`;
    expect(apiBase()).toBe("/apps/transit-reach/api");
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"a/b"}</script>`;
    expect(apiBase()).toBe("/apps/transit-reach/api");
  });

  it("asks for a stop's areas with its id encoded, and reads a refusal's words", async () => {
    const fetch = vi.fn(async (_url: string, _init?: RequestInit) => answer(200, KEPT)).mockResolvedValueOnce(answer(200, KEPT)).mockResolvedValueOnce(answer(404, { detail: "HSL's network holds no such stop with a place" }));
    vi.stubGlobal("fetch", fetch);
    const api = reachApi("/a");
    await expect(api.reach("urn:ngsi-ld:GtfsStop:HSL:1&x")).resolves.toEqual(KEPT);
    expect(fetch.mock.calls[0][0]).toBe("/a/reach?stop=urn%3Angsi-ld%3AGtfsStop%3AHSL%3A1%26x");
    await expect(api.reach("urn:ngsi-ld:GtfsStop:HSL:9")).rejects.toMatchObject({ status: 404, message: "HSL's network holds no such stop with a place" });
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(api.refresh()).rejects.toMatchObject({ status: 0 });
  });
});

// T-3826: who is signed in, the origins and the history.
describe("reachApi, origins", () => {
  it("calls each of the server's paths with its method and body, and reads a 204 as nothing", async () => {
    const fetch = vi.fn(async (_url: string, init?: RequestInit) => (["PUT", "DELETE"].includes(init?.method ?? "") ? new Response(null, { status: 204 }) : answer(200, null)));
    vi.stubGlobal("fetch", fetch);
    const api = reachApi("/a");
    expect(await api.me()).toBeNull();
    await api.origins();
    await api.keep({ name: "Kamppi", lon: 24.93, lat: 60.17, source: "network", bands: [] });
    expect(await api.rename(3, "Home")).toBeUndefined();
    await api.forget(3);
    await api.publish(3, true);
    await api.history();
    expect(fetch.mock.calls.map(([url, init]) => `${init?.method} ${url} ${init?.body ?? ""}`)).toEqual([
      "GET /a/me ",
      "GET /a/origins ",
      'POST /a/origins {"name":"Kamppi","lon":24.93,"lat":60.17,"source":"network","bands":[]}',
      'PUT /a/origins/3 {"name":"Home"}',
      "DELETE /a/origins/3 ",
      'PUT /a/origins/3/published {"published":true}',
      "GET /a/history ",
    ]);
    vi.stubGlobal("fetch", vi.fn(async () => new Response("<html>", { status: 403 })));
    await expect(api.publish(3, false)).rejects.toMatchObject({ status: 403, message: "" });
  });
});

describe("keptReach", () => {
  const api = (reach: ReachApi["reach"], refresh: ReachApi["refresh"] = vi.fn(async () => ({ version: "v1", stops: 3, routes: 2, changed: true }))): Pick<ReachApi, "reach" | "refresh"> => ({ reach, refresh });

  it("asks the server to read HSL's network once when it has none, then asks again", async () => {
    const reach = vi.fn<ReachApi["reach"]>().mockRejectedValueOnce(new ServerProblem(409, "not read yet")).mockResolvedValueOnce(KEPT);
    const server = api(reach);
    await expect(keptReach(server, KEPT.stop)).resolves.toEqual(KEPT);
    expect(server.refresh).toHaveBeenCalledTimes(1);
    expect(reach).toHaveBeenCalledTimes(2);
  });

  it("reads a network due again in the background, and passes any other refusal on", async () => {
    const server = api(vi.fn(async () => ({ ...KEPT, stale: true })), vi.fn().mockRejectedValue(new ServerProblem(502, "")));
    await expect(keptReach(server, KEPT.stop)).resolves.toMatchObject({ stale: true });
    expect(server.refresh).toHaveBeenCalledTimes(1);
    const refused = api(vi.fn().mockRejectedValue(new ServerProblem(403, "")));
    await expect(keptReach(refused, KEPT.stop)).rejects.toMatchObject({ status: 403 });
    expect(refused.refresh).not.toHaveBeenCalled();
  });
});
