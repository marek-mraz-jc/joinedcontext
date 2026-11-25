// T-7706: readServer says what a server's refusal was: a plain HTTP failure, and a JSON-RPC error with and without words.
import { afterEach, describe, expect, it, vi } from "vitest";
import { readServer } from "../src/pages/connect/connect";

afterEach(() => vi.unstubAllGlobals());

const stub = (response: Response) => vi.stubGlobal("fetch", vi.fn(async () => response));

describe("readServer", () => {
  it("names the status of a server that answers with a failure", async () => {
    stub(new Response("oops", { status: 500 }));
    expect(await readServer("helsinki", "geo-tools")).toEqual({ state: "failed", reason: "HTTP 500" });
  });

  it("gives the words of a JSON-RPC error, or a plain word when it has none", async () => {
    stub(new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, error: { code: -32000, message: "the tool host is paused" } }), { status: 200, headers: { "Content-Type": "application/json" } }));
    expect(await readServer("helsinki", "geo-tools")).toEqual({ state: "failed", reason: "the tool host is paused" });
    stub(new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, error: { code: -32000 } }), { status: 200, headers: { "Content-Type": "application/json" } }));
    expect(await readServer("helsinki", "geo-tools")).toEqual({ state: "failed", reason: "error" });
  });
});
