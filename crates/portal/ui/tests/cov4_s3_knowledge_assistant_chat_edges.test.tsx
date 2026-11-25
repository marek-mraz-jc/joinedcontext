// T-7706: the assistant chat while it streams (stages, tool reads, delta text), an empty or busy send, a CSV with no columns, a refused copy.
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";

vi.mock("../src/components/panels/VegaChart", async (actual) => ({
  ...(await actual<typeof import("../src/components/panels/VegaChart")>()),
  VegaChart: ({ label }: { label: string }) => <div role="img" aria-label={label} />,
}));
import { App } from "../src/App";

const GRANTS = {
  project: "banskabystrica",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "people", scope: "project:banskabystrica", rule: { kinds: ["KnowledgeSource", "AssistantDeployment"], verbs: ["read", "propose"] } }],
};
const DEPLOYMENT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "AssistantDeployment",
  metadata: { name: "obcania", namespace: "banskabystrica" },
  spec: { publicId: "bb-obcania", channel: "public", languages: ["sk"], sources: ["bb-web"], connectors: [{ endpoint: "ovzdusie-verejne", tools: ["query_entities"] }] },
};

interface Chat {
  body: string;
  hold?: boolean;
}

function renderChat(chat: Chat) {
  const asked: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana", roles: [] });
      if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", domain: "dev.example", languages: { default: "en", offered: ["en"] } });
      if (url.pathname.endsWith("/permissions/me")) return json(GRANTS);
      if (request.method === "POST" && url.pathname.endsWith("/chat")) {
        asked.push(await request.json());
        if (chat.hold) {
          const body = new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(new TextEncoder().encode(chat.body));
              request.signal.addEventListener("abort", () => controller.error(new DOMException("aborted", "AbortError")));
            },
          });
          return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
        }
        return new Response(chat.body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
      }
      if (url.pathname === "/api/v1/projects/banskabystrica/assistantdeployments") {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [DEPLOYMENT] });
      }
      if (url.pathname.endsWith("/knowledge/sources")) return json({ items: [] });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  window.history.pushState({}, "", "/projects/banskabystrica/knowledge");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return asked;
}

async function openChat(): Promise<HTMLElement> {
  const table = await screen.findByRole("table", { name: "Assistants" });
  await userEvent.click(within(table).getByRole("button", { name: "Ask obcania" }));
  return screen.findByRole("dialog", { name: "Ask obcania" });
}

const event = (name: string, data: unknown) => `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("an answer under way", () => {
  it("shows the live step, the steps already taken and the text so far, and stops on request", async () => {
    renderChat({
      hold: true,
      body:
        event("stage", { stage: "understanding" }) +
        event("stage", { stage: "searching", topic: "odpad" }),
    });
    const dialog = await openChat();
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Your question" }), "Odpad?{Enter}");
    expect(await within(dialog).findByText("Searching the sources: „odpad“…")).toBeInTheDocument();
    expect(dialog.textContent).toContain("✓ Reading your question");
    // Nothing more is asked while the answer is under way.
    const form = within(dialog).getByRole("textbox", { name: "Your question" }).closest("form") as HTMLFormElement;
    fireEvent.submit(form);
    await userEvent.click(within(dialog).getByRole("button", { name: "Stop" }));
    expect(await within(dialog).findByText("You stopped the answer. What arrived stays.")).toBeInTheDocument();
  });

  it("shows the text so far with its open marker cut, and stops on request", async () => {
    renderChat({ hold: true, body: event("delta", { text: "Pozri [kalendár](https://elsewhere.example/odpad) a odpad vyvážame v **utor" }) });
    const dialog = await openChat();
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Your question" }), "Odpad?{Enter}");
    expect(await within(dialog).findByText(/odpad vyvážame v utor/)).toBeInTheDocument();
    expect(dialog.textContent).not.toContain("**");
    // A link to a host the deployment does not know stays its words while the answer streams.
    expect(within(dialog).queryByRole("link", { name: "kalendár" })).not.toBeInTheDocument();
    expect(dialog.textContent).toContain("kalendár");
    await userEvent.click(within(dialog).getByRole("button", { name: "Stop" }));
    expect(await within(dialog).findByText("You stopped the answer. What arrived stays.")).toBeInTheDocument();
  });

  it("names the endpoint being read while no stage has a line of its own", async () => {
    renderChat({
      hold: true,
      body: event("tool", { name: "query_entities", endpoint: "ovzdusie-verejne", status: "started" }),
    });
    const dialog = await openChat();
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Your question" }), "PM10?{Enter}");
    expect(await within(dialog).findByText("Reading ovzdusie-verejne…")).toBeInTheDocument();
  });

  it("sends nothing for an empty question", async () => {
    const asked = renderChat({ body: event("answer", { text: "x" }) });
    const dialog = await openChat();
    const form = within(dialog).getByRole("textbox", { name: "Your question" }).closest("form") as HTMLFormElement;
    fireEvent.submit(form);
    expect(asked).toEqual([]);
    expect(within(dialog).getByRole("button", { name: "Send" })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("a table of data", () => {
  it("writes the rows as a CSV file when asked", async () => {
    const create = vi.fn(() => "blob:rows");
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: create });
    Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
    renderChat({
      body:
        event("data", { type: "Air", columns: ["station", "pm10"], rows: [["A", 3]] }) +
        event("answer", { text: "Hotovo." }) +
        event("done", { tokens: 3 }),
    });
    const dialog = await openChat();
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Your question" }), "PM10?{Enter}");
    await within(dialog).findByText("Hotovo.");
    await userEvent.click(await within(dialog).findByRole("button", { name: "Download CSV" }));
    expect(create).toHaveBeenCalledOnce();
  });
});

describe("the embed snippet", () => {
  it("keeps the copy button as it was when the clipboard refuses", async () => {
    renderChat({ body: "" });
    const table = await screen.findByRole("table", { name: "Assistants" });
    await userEvent.click(within(table).getByRole("button", { name: "Embed obcania" }));
    const dialog = await screen.findByRole("dialog", { name: "Embed obcania" });
    const writeText = vi.fn(async () => {
      throw new Error("denied");
    });
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const copy = within(dialog).getByRole("button", { name: "Copy The launcher line" });
    await userEvent.click(copy);
    await waitFor(() => expect(writeText).toHaveBeenCalledOnce());
    expect(copy).toHaveTextContent("Copy");
    expect(copy).not.toHaveTextContent("Copied");
  });
});
