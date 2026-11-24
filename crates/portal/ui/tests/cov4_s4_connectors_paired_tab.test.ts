// T-7706: an action that reaches a connected tab while no bar is mounted waits, and runs when the bar attaches.
import { afterEach, describe, expect, it, vi } from "vitest";
import { doorsAsRest } from "./odata";
import { attach, connectTab, disconnectTab, tabLog } from "../src/connectors/pairedTab";

const TAB = "0123456789abcdef0123456789abcdef";

class FakeSource {
  static CLOSED = 2;
  static opened: FakeSource[] = [];
  readyState = 1;
  onerror: (() => void) | null = null;
  listeners = new Map<string, (event: MessageEvent<string>) => void>();
  constructor(public url: string) {
    FakeSource.opened.push(this);
  }
  addEventListener(kind: string, listener: (event: MessageEvent<string>) => void) {
    this.listeners.set(kind, listener);
  }
  close() {
    this.readyState = FakeSource.CLOSED;
  }
  send(data: unknown) {
    this.listeners.get("action")?.(new MessageEvent("action", { data: JSON.stringify(data) }));
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("a connected tab with no bar mounted", () => {
  it("holds the client's action until a bar attaches, then opens its page in order", async () => {
    vi.stubGlobal("EventSource", FakeSource);
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        const url = new URL(request.url, "http://localhost");
        const json = (body: unknown, status = 200) =>
          new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
        if (url.pathname === "/api/v1/connectors/abc/tab" && request.method === "POST") {
          return json({ tab: TAB, connector: "abc", name: "doprava-in-claude", until: "2026-10-29T12:00:00Z" }, 201);
        }
        return json(null, 204);
      }),
    );
    doorsAsRest();
    await connectTab("abc");
    const stream = FakeSource.opened.at(-1);
    expect(stream?.url).toContain(`/tab/${TAB}/events`);
    const navigate = (route: string) => ({ frame: "navigate", tool: "jc_ui_open_form", payload: { route }, from: { connector: "doprava-in-claude" } });
    stream?.send(navigate("/projects/doprava/pipelines"));
    stream?.send(navigate("/projects/doprava/endpoints"));
    expect(tabLog()).toEqual([]);

    const opened: string[] = [];
    const detach = attach((href) => opened.push(href), () => undefined);
    await vi.waitFor(() => expect(tabLog()).toHaveLength(2));
    expect(opened).toEqual(["/projects/doprava/pipelines", "/projects/doprava/endpoints"]);
    detach();
    await disconnectTab();
  });
});
