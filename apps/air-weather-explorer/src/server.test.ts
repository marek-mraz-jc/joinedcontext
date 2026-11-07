import { afterEach, describe, expect, it, vi } from "vitest";
import { airApi, apiBase, compareLink, isCode } from "./server";

function answer(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

// T-3348: the page reads the kept hours and keeps comparisons through its own server.
describe("airApi", () => {
  it("names the App's own server, never a name that leaves its path", () => {
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"air-weather-explorer"}</script>`;
    expect(apiBase()).toBe("/apps/air-weather-explorer/api");
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"../x"}</script>`;
    expect(apiBase()).toBe("/apps/air-weather-explorer/api");
  });

  it("asks for both stations' hours with their ids encoded", async () => {
    const fetch = vi.fn(async (_url: string, _init?: RequestInit) => answer(200, { air: {}, weather: {} }));
    vi.stubGlobal("fetch", fetch);
    await airApi("/a").series("urn:ngsi-ld:AirQualityObserved:x:1", "urn:ngsi-ld:WeatherObserved:y&z", 7);
    expect(fetch.mock.calls[0][0]).toBe("/a/series?air=urn%3Angsi-ld%3AAirQualityObserved%3Ax%3A1&weather=urn%3Angsi-ld%3AWeatherObserved%3Ay%26z&days=7");
  });

  it("saves, opens and exports, and reads a refusal's words", async () => {
    const fetch = vi
      .fn(async (_url: string, _init?: RequestInit) => answer(200, {}))
      .mockResolvedValueOnce(answer(201, { code: "abc123def456", name: null }))
      .mockResolvedValueOnce(answer(200, { code: "abc123def456", station: "s", weather: "w", days: 3, smoothing: 3, air: null, variable: null, name: null }))
      .mockResolvedValueOnce(answer(201, { url: "https://store.example/e.csv" }))
      .mockResolvedValueOnce(answer(400, { detail: "the smoothing is 1 to 12 hours" }));
    vi.stubGlobal("fetch", fetch);
    const api = airApi("/a");
    const comparison = { name: null, station: "s", weather: "w", days: 3, smoothing: 3, air: null, variable: null };
    await expect(api.save(comparison)).resolves.toEqual({ code: "abc123def456", name: null });
    await expect(api.open("abc123def456")).resolves.toMatchObject({ station: "s" });
    await expect(api.exportUrl("s", "w", 3)).resolves.toBe("https://store.example/e.csv");
    expect(JSON.parse(fetch.mock.calls[2][1]?.body as string)).toEqual({ air: "s", weather: "w", days: 3 });
    await expect(api.save({ ...comparison, smoothing: 0 })).rejects.toMatchObject({ status: 400, message: "the smoothing is 1 to 12 hours" });
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(api.open("abc123def456")).rejects.toMatchObject({ status: 0 });
  });

  // T-3827: who calls, their places, a steward's marks and the history, each at its route.
  it("calls each route of the roles with its method and body, and reads a 204 as nothing", async () => {
    const fetch = vi.fn(async (_url: string, init?: RequestInit) => (init?.method === "PUT" || init?.method === "DELETE" ? new Response(null, { status: 204 }) : answer(200, [])));
    vi.stubGlobal("fetch", fetch);
    const api = airApi("/a");
    await api.me();
    await api.places();
    await api.addPlace({ station: "s", name: "Koti", attr: "pm25", threshold: 10 });
    await expect(api.changePlace(3, "Koti", 12)).resolves.toBeUndefined();
    await api.removePlace(3);
    await api.faults("urn:ngsi-ld:AirQualityObserved:a&b");
    await api.markFault({ station: "s", from: 1, to: 2, note: "x" });
    await api.unmarkFault(4);
    await api.history();
    expect(fetch.mock.calls.map(([url, init]) => `${init?.method} ${url}`)).toEqual([
      "GET /a/me",
      "GET /a/places",
      "POST /a/places",
      "PUT /a/places/3",
      "DELETE /a/places/3",
      "GET /a/faults?station=urn%3Angsi-ld%3AAirQualityObserved%3Aa%26b",
      "POST /a/faults",
      "DELETE /a/faults/4",
      "GET /a/history",
    ]);
    expect(JSON.parse(fetch.mock.calls[3][1]?.body as string)).toEqual({ name: "Koti", threshold: 12 });
  });

  it("takes only a code the server could have made, and links the page with it alone", () => {
    expect(isCode("abc123def456")).toBe(true);
    for (const wrong of ["", "abc", "ABC123DEF456", "abc123def45/"]) expect(isCode(wrong)).toBe(false);
    expect(compareLink("abc123def456", { origin: "https://dev.example", pathname: "/apps/air-weather-explorer/" })).toBe("https://dev.example/apps/air-weather-explorer/?compare=abc123def456");
  });
});
