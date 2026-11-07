import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, kpiApi } from "./server";

function answer(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

// T-3350: the page records and reads its forecasts through its own server.
describe("kpiApi", () => {
  it("names the App's own server, never a name that leaves its path", () => {
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"kpi-forecast"}</script>`;
    expect(apiBase()).toBe("/apps/kpi-forecast/api");
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"x?y"}</script>`;
    expect(apiBase()).toBe("/apps/kpi-forecast/api");
  });

  it("records a period, lists an indicator's forecasts with its id encoded, and asks for a month's report", async () => {
    const fetch = vi
      .fn(async (_url: string, _init?: RequestInit) => answer(200, []))
      .mockResolvedValueOnce(answer(201, { recorded: 4, already: false }))
      .mockResolvedValueOnce(answer(200, []))
      .mockResolvedValueOnce(answer(201, { url: "https://store.example/r.csv" }));
    vi.stubGlobal("fetch", fetch);
    const api = kpiApi("/a");
    await expect(api.record(30)).resolves.toEqual({ recorded: 4, already: false });
    await expect(api.list("urn:ngsi-ld:KeyPerformanceIndicator:a&b")).resolves.toEqual([]);
    await expect(api.reportUrl("2026-10")).resolves.toBe("https://store.example/r.csv");
    expect(fetch.mock.calls.map((c) => c[0])).toEqual(["/a/forecasts", "/a/forecasts?kpi=urn%3Angsi-ld%3AKeyPerformanceIndicator%3Aa%26b", "/a/reports"]);
    expect(JSON.parse(fetch.mock.calls[0][1]?.body as string)).toEqual({ days: 30 });
    expect(JSON.parse(fetch.mock.calls[2][1]?.body as string)).toEqual({ month: "2026-10" });
  });

  it("reads a refusal's words, and a dead network as status 0", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => answer(400, { detail: "that month has not begun" })));
    await expect(kpiApi("/a").reportUrl("2099-01")).rejects.toMatchObject({ status: 400, message: "that month has not begun" });
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(kpiApi("/a").record(7)).rejects.toMatchObject({ status: 0 });
  });

  // T-3827: who calls, a steward's published forecasts, the reader's follows and the history.
  it("calls each route of the roles with its method and body, and reads a 204 as nothing", async () => {
    const fetch = vi.fn(async (_url: string, init?: RequestInit) => (init?.method === "DELETE" ? new Response(null, { status: 204 }) : answer(200, [])));
    vi.stubGlobal("fetch", fetch);
    const api = kpiApi("/a");
    await api.me();
    await api.published("urn:ngsi-ld:KeyPerformanceIndicator:a&b");
    await api.publish("k", 30, 6, "note");
    await expect(api.withdraw(3)).resolves.toBeUndefined();
    await api.follows();
    await api.follow("k");
    await api.unfollow(4);
    await api.history();
    expect(fetch.mock.calls.map(([url, init]) => `${init?.method} ${url}`)).toEqual([
      "GET /a/me",
      "GET /a/published?kpi=urn%3Angsi-ld%3AKeyPerformanceIndicator%3Aa%26b",
      "POST /a/published",
      "DELETE /a/published/3",
      "GET /a/follows",
      "POST /a/follows",
      "DELETE /a/follows/4",
      "GET /a/history",
    ]);
    expect(JSON.parse(fetch.mock.calls[2][1]?.body as string)).toEqual({ kpi: "k", days: 30, horizon: 6, note: "note" });
  });
});
