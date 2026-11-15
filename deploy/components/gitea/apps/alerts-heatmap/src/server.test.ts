import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, Problem, server } from "./server";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the App's server", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("answers at /apps/{name}/api, and a name that is not an App's never builds a path", () => {
    expect(apiBase("alerts-heatmap")).toBe("/apps/alerts-heatmap/api");
    expect(apiBase(undefined)).toBe("/api");
    expect(apiBase("../x")).toBe("/api");
  });

  it("reads and writes JSON, and an answer that is not a success is a Problem with its detail", async () => {
    const fetch = vi.fn(async (url: string, init?: RequestInit) => {
      if (url.endsWith("/weeks")) return json(200, { weeks: [], stale: false });
      if (url.endsWith("/reports") && init?.method === "POST") return json(400, { title: "Bad Request", detail: "a report's title has 1 to 120 characters" });
      return json(502, {});
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/apps/a/api");
    expect(await api.weeks()).toEqual({ weeks: [], stale: false });
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ method: "GET", credentials: "same-origin" });
    const refused = await api.save({ title: "", view: "", kept: 0, places: [] }).catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(Problem);
    expect(refused).toMatchObject({ status: 400, message: "a report's title has 1 to 120 characters" });
    expect(JSON.parse(String(fetch.mock.calls[1]?.[1]?.body))).toEqual({ title: "", view: "", kept: 0, places: [] });
    await expect(api.reports()).rejects.toMatchObject({ status: 502, message: "HTTP 502" });
  });

  it("puts a picture to the URL the server hands out, and says when the store refuses it", async () => {
    const puts: Array<[string, RequestInit | undefined]> = [];
    let store = 200;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/api/reports/3/snapshot" && init?.method === "POST") return json(200, { url: "https://store.test/put", method: "PUT" });
        if (url === "/api/reports/3/snapshot") return json(200, { url: "https://store.test/get" });
        puts.push([url, init]);
        return new Response(null, { status: store });
      }),
    );
    const api = server("/api");
    const png = new Blob(["png"], { type: "image/png" });
    await api.attach(3, png);
    expect(puts[0]?.[0]).toBe("https://store.test/put");
    expect(puts[0]?.[1]).toMatchObject({ method: "PUT", body: png, headers: { "Content-Type": "image/png" } });
    store = 403;
    await expect(api.attach(3, png)).rejects.toMatchObject({ status: 403 });
    expect(await api.snapshotUrl(3)).toBe("https://store.test/get");
  });

  it("asks who reads, keeps and forgets filters, acknowledges alerts and reads the history (T-3828)", async () => {
    const calls: Array<[string, string]> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const method = init?.method ?? "GET";
        calls.push([method, url]);
        if (method === "DELETE") return new Response(null, { status: 204 });
        if (url.endsWith("/me")) return json(200, null);
        if (url.endsWith("/filters") && method === "POST") return json(201, { id: 4, ...JSON.parse(String(init?.body)) });
        if (url.endsWith("/acks") && method === "POST") return json(201, { id: 9, ownerName: "Sara", ...JSON.parse(String(init?.body)) });
        return json(200, []);
      }),
    );
    const api = server("/api");
    expect(await api.me()).toBeNull();
    expect(await api.filters()).toEqual([]);
    expect(await api.keepFilter({ name: "Road works", view: "kinds=roadWorks" })).toEqual({ id: 4, name: "Road works", view: "kinds=roadWorks" });
    expect(await api.forgetFilter(4)).toBeUndefined();
    expect(await api.acks()).toEqual([]);
    const ack = await api.acknowledge("urn:ngsi-ld:Alert:1", "Crew sent");
    expect(ack).toMatchObject({ id: 9, alert: "urn:ngsi-ld:Alert:1", comment: "Crew sent", ownerName: "Sara" });
    expect(Number.isNaN(Date.parse(ack.at))).toBe(false);
    expect(await api.unacknowledge(9)).toBeUndefined();
    expect(await api.changes()).toEqual([]);
    expect(calls).toEqual([
      ["GET", "/api/me"],
      ["GET", "/api/filters"],
      ["POST", "/api/filters"],
      ["DELETE", "/api/filters/4"],
      ["GET", "/api/acks"],
      ["POST", "/api/acks"],
      ["DELETE", "/api/acks/9"],
      ["GET", "/api/history"],
    ]);
  });
});
