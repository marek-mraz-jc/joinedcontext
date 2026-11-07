import { afterEach, describe, expect, it, vi } from "vitest";
import { apiBase, Problem, server } from "./server";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("the App's own server", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("answers under /apps/{name}/api, and /api for a name that is not an App's", () => {
    expect(apiBase("bookings")).toBe("/apps/bookings/api");
    expect(apiBase("../x")).toBe("/api");
    expect(apiBase(undefined)).toBe("/api");
  });

  it("lists and adds items, and throws the server's detail on a refusal", async () => {
    const fetch = vi.fn(async (url: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        const { text } = JSON.parse(String(init.body)) as { text: string };
        return text ? json(201, { id: 2, text, created_at: "2026-10-10" }) : json(422, { detail: "text is empty" });
      }
      return url === "/api/items" ? json(200, [{ id: 1, text: "a", created_at: "2026-10-09" }]) : json(404, {});
    });
    vi.stubGlobal("fetch", fetch);
    const api = server("/api");
    expect((await api.items()).map((item) => item.text)).toEqual(["a"]);
    expect((await api.addItem("b")).id).toBe(2);
    await expect(api.addItem("")).rejects.toEqual(new Problem(422, "text is empty"));
  });
});
