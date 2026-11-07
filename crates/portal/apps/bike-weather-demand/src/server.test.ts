import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, Problem, server } from "./server";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the App's server", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("answers at /apps/{name}/api, and a name that is not an App's never builds a path", () => {
    expect(apiBase("bike-weather-demand")).toBe("/apps/bike-weather-demand/api");
    expect(apiBase(undefined)).toBe("/api");
    expect(apiBase("../x")).toBe("/api");
  });

  it("asks for a station's models and a model's training data, and an answer that is not a success is a Problem with its detail", async () => {
    const fetch = vi.fn(async (url: string, _init?: RequestInit) => {
      if (url === "/api/models?station=urn%3Angsi-ld%3AB%3A1") return json(200, { models: [], stale: false });
      if (url === "/api/models/3/snapshot") return json(200, { url: "https://store.test/t.json" });
      if (url === "/api/models/4/snapshot") return json(404, { title: "Not Found", detail: "no such model" });
      return new Response("<html>", { status: 502 });
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect(await api.models("urn:ngsi-ld:B:1")).toEqual({ models: [], stale: false });
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ credentials: "same-origin" });
    expect(await api.snapshotUrl(3)).toBe("https://store.test/t.json");
    const refused = await api.snapshotUrl(4).catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(Problem);
    expect(refused).toMatchObject({ status: 404, message: "no such model" });
    await expect(api.snapshotUrl(5)).rejects.toMatchObject({ status: 502, message: "HTTP 502" });
  });

  it("reads who is signed in, sets a station's inputs as JSON, resets them, and reads the history", async () => {
    const station = "urn:ngsi-ld:B:1";
    const at = `/api/inputs?station=${encodeURIComponent(station)}`;
    const fetch = vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/api/me") return json(200, null);
      if (url === at && init?.method === "PUT") return json(200, { ...JSON.parse(String(init.body)), station, custom: true });
      if (url === at && init?.method === "DELETE") return new Response(null, { status: 204 });
      if (url === at) return json(200, { station, custom: false });
      if (url === "/api/history") return json(403, { detail: "this needs the role steward" });
      return new Response("<html>", { status: 502 });
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect(await api.me()).toBeNull();
    expect(await api.inputs(station)).toEqual({ station, custom: false });
    const set = { weatherStation: null, radiusKm: 5, days: 3, note: "n" };
    expect(await api.setInputs(station, set)).toMatchObject({ ...set, custom: true });
    expect(fetch.mock.calls[2]?.[1]).toMatchObject({ method: "PUT", headers: { "Content-Type": "application/json" } });
    expect(await api.resetInputs(station)).toBeUndefined();
    await expect(api.history()).rejects.toMatchObject({ status: 403, message: "this needs the role steward" });
  });
});
