import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, isCode, shareApi, shareLink } from "./share";

function answer(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

// T-3347: a day shared on the App's own server, under a code a link carries.
describe("shareApi", () => {
  it("names the App's own server, never a name that leaves its path", () => {
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"event-day-planner"}</script>`;
    expect(apiBase()).toBe("/apps/event-day-planner/api");
    document.body.innerHTML = `<script id="jc-config" type="application/json">{"appName":"x/../../portal"}</script>`;
    expect(apiBase()).toBe("/apps/event-day-planner/api");
  });

  it("takes only a code the server could have made, and links the page with it alone", () => {
    expect(isCode("abc123def456")).toBe(true);
    for (const wrong of ["", "abc", "ABC123DEF456", "abc123def45/", "abc123def4567"]) expect(isCode(wrong)).toBe(false);
    expect(shareLink("abc123def456", { origin: "https://dev.example", pathname: "/apps/event-day-planner/" })).toBe("https://dev.example/apps/event-day-planner/?share=abc123def456");
  });

  it("sends the day, the full ids and the language, and reads a refusal's words", async () => {
    const fetch = vi.fn(async (_url: string, _init?: RequestInit) => answer(201, { code: "abc123def456" }));
    vi.stubGlobal("fetch", fetch);
    await shareApi("/a").share("2030-10-20", ["urn:ngsi-ld:Event:x:1"], "fi");
    expect(fetch.mock.calls[0][0]).toBe("/a/itineraries");
    expect(JSON.parse(fetch.mock.calls[0][1]?.body as string)).toEqual({ day: "2030-10-20", ids: ["urn:ngsi-ld:Event:x:1"], lang: "fi" });
    vi.stubGlobal("fetch", vi.fn(async () => answer(409, { detail: "none of the picked events takes place that day" })));
    await expect(shareApi("/a").share("2030-10-20", ["x"], "en")).rejects.toMatchObject({ status: 409, message: "none of the picked events takes place that day" });
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(shareApi("/a").get("abc123def456")).rejects.toMatchObject({ status: 0 });
  });

  it("reads a config it cannot parse as the App's own name", () => {
    document.body.innerHTML = `<script id="jc-config" type="application/json">{not json</script>`;
    expect(apiBase()).toBe("/apps/event-day-planner/api");
  });

  // T-3826: who is signed in, their kept days, the featured events and the history.
  it("calls each of the server's paths with its method and body, and reads a 204 as nothing", async () => {
    const fetch = vi.fn(async (_url: string, init?: RequestInit) => (init?.method === "PUT" || init?.method === "DELETE" ? new Response(null, { status: 204 }) : answer(200, null)));
    vi.stubGlobal("fetch", fetch);
    const api = shareApi("/a");
    expect(await api.me()).toBeNull();
    await api.days();
    expect(await api.forgetDay(3)).toBeUndefined();
    await api.featured();
    await api.feature("urn:ngsi-ld:Event:x:1", "Free entry");
    await api.changeNote(4, "Bring a coat");
    await api.unfeature(4);
    await api.history();
    expect(fetch.mock.calls.map(([url, init]) => `${init?.method} ${url} ${init?.body ?? ""}`)).toEqual([
      "GET /a/me ",
      "GET /a/days ",
      "DELETE /a/days/3 ",
      "GET /a/featured ",
      'POST /a/featured {"eventId":"urn:ngsi-ld:Event:x:1","note":"Free entry"}',
      'PUT /a/featured/4 {"note":"Bring a coat"}',
      "DELETE /a/featured/4 ",
      "GET /a/history ",
    ]);
    vi.stubGlobal("fetch", vi.fn(async () => new Response("<html>", { status: 403 })));
    await expect(api.unfeature(4)).rejects.toMatchObject({ status: 403, message: "" });
  });
});
