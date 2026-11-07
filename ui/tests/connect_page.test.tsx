// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/connect/ConnectPage.tsx and
// src/pages/connect/connect.ts, the public setup page of one named MCP server (T-4169, T-4141).
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import { ConnectPage } from "../src/pages/connect/ConnectPage";
import { clientName, serverUrl, setups } from "../src/pages/connect/connect";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const TITLE = "Banská Bystrica Region, open data";
const ABOUT = "Read-only open data of the Banská Bystrica Self-Governing Region (BBSK), Slovakia.";
// What the gateway's `initialize` and anonymous `tools/list` answer for a named server
// (context-gateway mcp/server.rs `initialized`), cut to two tools.
const HELLO = {
  jsonrpc: "2.0",
  id: 1,
  result: {
    protocolVersion: "2025-06-18",
    serverInfo: { name: "otvorene-data", title: TITLE, version: "0.1.0" },
    instructions: `${ABOUT}\n\nOne server over several Endpoints. list_endpoints names the ones you may read.`,
  },
};
const TOOLS = {
  jsonrpc: "2.0",
  id: 1,
  result: {
    tools: [
      { name: "list_types", description: "The entity types every member serves.\nMore.", annotations: { readOnlyHint: true } },
      { name: "query_entities", description: "Entities of one type.", annotations: { readOnlyHint: true } },
    ],
  },
};

type Reply = { status: number; body?: unknown; headers?: Record<string, string> };

function serve(replies: Record<string, Reply>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const { method } = (await request.clone().json()) as { method: string };
    const reply = replies[method] ?? { status: 500 };
    return new Response(reply.body === undefined ? "" : JSON.stringify(reply.body), {
      status: reply.status,
      headers: { "Content-Type": "application/json", ...reply.headers },
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

const show = () =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <ConnectPage project="bbsk" name="otvorene-data" />
      </I18nextProvider>
    </QueryClientProvider>,
  );

describe("the public MCP connect page (T-4169)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("names the server, its tools and one setup per client, asked as a stranger", async () => {
    const fetchMock = serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    const { container } = show();
    expect(await screen.findByText(TITLE)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: en.connect.title })).toBeInTheDocument();
    // The server's own words, without the gateway's note on fan-out.
    expect(screen.getByText(ABOUT)).toBeInTheDocument();
    expect(screen.queryByText(/One server over several Endpoints/)).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "2 tools" })).toBeInTheDocument();
    expect(screen.getByText("list_types")).toBeInTheDocument();
    expect(screen.getByText("The entity types every member serves.")).toBeInTheDocument();
    for (const client of ["Claude Code", "Claude Desktop", "ChatGPT", en.connect.generic.title]) {
      expect(screen.getByRole("heading", { level: 3, name: client })).toBeInTheDocument();
    }
    const url = `${window.location.origin}/api/mcp/bbsk/otvorene-data`;
    expect(screen.getByText(`claude mcp add --transport http bbsk-otvorene-data ${url}`)).toBeInTheDocument();
    expect(screen.getByText(/Up to 60 requests a minute/)).toBeInTheDocument();

    const requests = fetchMock.mock.calls.map(([input]) => input as Request);
    expect(requests).toHaveLength(2);
    for (const request of requests) {
      expect(request.url).toBe(url);
      expect(request.credentials).toBe("omit");
      expect(request.headers.get("authorization")).toBeNull();
    }
    expect(container.textContent).not.toMatch(/token|Bearer|\/api\/mcp"/i);
    await expectNoViolations(container);
  });

  it("copies a setup and says so to a screen reader", async () => {
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    show();
    fireEvent.click(await screen.findByRole("button", { name: en.connect.claudeCode.copy }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(expect.stringMatching(/^claude mcp add --transport http /)));
    expect(await screen.findByText(en.connect.copied)).toHaveAttribute("role", "status");
    // A setup longer than the page scrolls sideways, so a keyboard reaches it (axe scrollable-region-focusable).
    for (const block of document.querySelectorAll("pre")) expect(block).toHaveAttribute("tabindex", "0");
  });

  it("goes back to Copy after two seconds", async () => {
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: vi.fn(async () => undefined) } });
    show();
    const button = await screen.findByRole("button", { name: en.connect.claudeCode.copy });
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      fireEvent.click(button);
      expect(await screen.findByText(en.connect.copied)).toBeInTheDocument();
      await act(() => vi.advanceTimersByTimeAsync(2100));
      expect(screen.queryByText(en.connect.copied)).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([
    ["refuses", { writeText: () => Promise.reject(new DOMException("denied", "NotAllowedError")) }],
    ["is missing", undefined],
  ])("copies by the browser's copy command when the clipboard %s (T-4762)", async (_, clipboard) => {
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    vi.stubGlobal("navigator", { ...navigator, clipboard });
    let copied = "";
    const execCommand = vi.fn(() => {
      copied = document.querySelector("textarea")?.value ?? "";
      return true;
    });
    Object.defineProperty(document, "execCommand", { configurable: true, value: execCommand });
    show();
    fireEvent.click(await screen.findByRole("button", { name: en.connect.claudeCode.copy }));
    expect(await screen.findByText(en.connect.copied)).toHaveAttribute("role", "status");
    expect(execCommand).toHaveBeenCalledWith("copy");
    expect(copied).toMatch(/^claude mcp add --transport http bbsk-otvorene-data https?:\/\/\S+$/);
    expect(document.querySelector("textarea")).toBeNull();
  });

  it("selects the block and says press Ctrl+C when nothing can copy (T-4762)", async () => {
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: () => Promise.reject(new Error("denied")) } });
    Object.defineProperty(document, "execCommand", { configurable: true, value: vi.fn(() => false) });
    show();
    // Claude Desktop's and the generic block share the label; the generic one comes last.
    fireEvent.click((await screen.findAllByRole("button", { name: en.connect.generic.copy })).at(-1)!);
    expect(await screen.findByText(en.connect.selected)).toHaveAttribute("role", "status");
    expect(JSON.parse(String(window.getSelection()))).toEqual({
      mcpServers: { "bbsk-otvorene-data": { type: "http", url: expect.stringMatching(/\/api\/mcp\/bbsk\/otvorene-data$/) } },
    });
  });

  it("tells a visitor an unknown or private connector apart from a busy one", async () => {
    serve({ initialize: { status: 404 } });
    const first = show();
    expect(await screen.findByText(en.connect.missing)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.connect.retry })).not.toBeInTheDocument();
    first.unmount();

    const fetchMock = serve({ initialize: { status: 429, headers: { "Retry-After": "17" } } });
    show();
    expect(await screen.findByText("Too many requests from your network right now. Try again in 17 seconds.")).toBeInTheDocument();
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    fireEvent.click(screen.getByRole("button", { name: en.connect.retry }));
    expect(await screen.findByText(TITLE)).toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("speaks Slovak first", async () => {
    await i18n.changeLanguage("sk");
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    show();
    await screen.findByText(TITLE);
    expect(screen.getByRole("heading", { level: 1, name: sk.connect.title })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "2 nástroje" })).toBeInTheDocument();
  });

  it.each(SUPPORTED_LOCALES)("says everything in %s with no raw key", async (locale) => {
    await i18n.changeLanguage(locale);
    serve({ initialize: { status: 200, body: HELLO }, "tools/list": { status: 200, body: TOOLS } });
    const { container } = show();
    await screen.findByText(TITLE);
    expectNoRawKeys(container);
  });
});

describe("the setups (T-4169)", () => {
  it("hands out the data host, never the Portal's, and nothing but the address", () => {
    expect(serverUrl("portal.dev.joinedcontext.com", "bbsk", "otvorene-data")).toBe(
      "https://api.dev.joinedcontext.com/api/mcp/bbsk/otvorene-data",
    );
    // Any other host is not known to be the platform's; the page's own origin serves the route.
    expect(serverUrl("localhost", "bbsk", "otvorene-data")).toBe(`${window.location.origin}/api/mcp/bbsk/otvorene-data`);
    expect(clientName("banskabystrica", "otvorene-data")).toBe("banskabystrica-otvorene-data");
    const url = "https://api.dev.joinedcontext.com/api/mcp/bbsk/otvorene-data";
    const lines = setups(url, "bbsk-otvorene-data");
    expect(JSON.parse(lines.generic)).toEqual({ mcpServers: { "bbsk-otvorene-data": { type: "http", url } } });
    expect(JSON.parse(lines.claudeDesktop)).toEqual({
      mcpServers: { "bbsk-otvorene-data": { command: "npx", args: ["-y", "mcp-remote", url] } },
    });
    expect(lines.claudeCode).toBe(`claude mcp add --transport http bbsk-otvorene-data ${url}`);
  });
});
