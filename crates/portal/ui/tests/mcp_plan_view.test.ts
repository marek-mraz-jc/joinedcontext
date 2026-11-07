import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The MCP Apps plan view the Portal serves at ui://joinedcontext/plan-view.html (T-5960), run as
// a host frame runs it: the host posts the tool input and result, the view answers by postMessage.
const PAGE = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), "../../src/mcp/plan_view.html"),
  "utf8",
);
const SCRIPT = (/<script>([\s\S]*)<\/script>/.exec(PAGE) ?? ["", ""])[1].replace(
  "__PORTAL_ORIGIN__",
  JSON.stringify("https://portal.example.org"),
);

type Message = { method?: string; id?: number; params?: Record<string, unknown> };
let sent: Message[] = [];

function post(data: unknown) {
  window.dispatchEvent(new MessageEvent("message", { data, source: window }));
}

function start(project: unknown, result: unknown) {
  document.body.innerHTML = '<main id="view"></main>';
  new Function(SCRIPT)();
  post({ jsonrpc: "2.0", id: 1, result: { hostContext: { theme: "light" } } });
  post({ jsonrpc: "2.0", method: "ui/notifications/tool-input", params: { arguments: { project } } });
  post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result });
}

const PLAN = {
  structuredContent: {
    planId: "p1",
    lane: "yellow",
    expiresAt: "2026-10-28T23:00:00Z",
    diff: [
      {
        op: "replace",
        kind: "Endpoint",
        name: "counters",
        lane: "yellow",
        plan: { fields: [{ path: "spec.title", from: "Old", to: '<img src=x onerror="alert(1)">' }] },
      },
    ],
    next: "call jc_change_apply with this planId within 30 minutes",
  },
};

beforeEach(() => {
  sent = [];
  vi.spyOn(window, "postMessage").mockImplementation((message: unknown) => {
    sent.push(message as Message);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the MCP Apps plan view", () => {
  it("greets the host, renders the diff as text and links to the Portal's approvals", () => {
    start("demo", PLAN);
    expect(sent[0].method).toBe("ui/initialize");
    expect(sent.some((m) => m.method === "ui/notifications/initialized")).toBe(true);
    expect(document.querySelector("h1")?.textContent).toContain("Plan of 1 change");
    const cells = [...document.querySelectorAll("td")].map((td) => td.textContent);
    expect(cells).toEqual(["spec.title", "Old", '<img src=x onerror="alert(1)">']);
    expect(document.querySelector("img")).toBeNull();
    const link = document.querySelector<HTMLAnchorElement>("a.button");
    expect(link?.getAttribute("href")).toBe("https://portal.example.org/projects/demo/approvals");
    link?.click();
    expect(sent.find((m) => m.method === "ui/open-link")?.params).toEqual({
      url: "https://portal.example.org/projects/demo/approvals",
    });
  });

  it("draws no link for a project that is not a project name", () => {
    for (const project of ["../evil", "https://evil.example", "a/b", "", 7]) {
      start(project, PLAN);
      expect(document.querySelector("a")).toBeNull();
    }
  });

  it("renders a mapping's sample records and its errors", () => {
    start("demo", {
      structuredContent: {
        records: [{ id: "urn:ngsi-ld:Counter:1", count: 4 }],
        errors: [{ stage: "mapping", line: 3, message: "no field 'x'" }],
      },
    });
    expect([...document.querySelectorAll("th")].map((th) => th.textContent)).toEqual(["id", "count"]);
    expect(document.querySelector("ul.errors")?.textContent).toBe("mapping · 3: no field 'x'");
    expect(document.querySelector("a.button")?.getAttribute("href")).toBe(
      "https://portal.example.org/projects/demo/pipelines",
    );
  });

  it("says a refusal in the tool's own words", () => {
    start("demo", { isError: true, content: [{ type: "text", text: "422: spec.title is required" }] });
    expect(document.querySelector("h1")?.textContent).toBe("The call was refused");
    expect(document.body.textContent).toContain("422: spec.title is required");
  });
});
