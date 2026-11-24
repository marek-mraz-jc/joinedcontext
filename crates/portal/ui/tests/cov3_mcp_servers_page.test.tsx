// T-7589: the MCP servers page: removing a server (and backing out), closing the dialog after a
// proposal, a project list that cannot be read, a clipboard that refuses.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { CopyValue } from "../src/pages/mcp/McpServerPanels";

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", roles: [] };
const GRANTS = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "steward", binding: "people", scope: "project:helsinki", rule: { kinds: ["McpServer", "Endpoint"], verbs: ["read", "propose", "delete"] } }],
};
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const SERVER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "McpServer",
  metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility data" } },
  spec: { members: [{ kind: "Endpoint", name: "bikes", namespace: "helsinki" }], audience: "organization" },
};
const BIKES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "helsinki", title: { en: "bikes" } },
  spec: { slug: "bikeslug", audience: "public", contextSpaceRef: { name: "helsinki" } },
};

function renderAt(options: { servers?: unknown[] } = {}) {
  const writes: { method: string; path: string }[] = [];
  const projects = { down: false };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, code = 200) =>
        Promise.resolve(new Response(JSON.stringify(body), { status: code, headers: { "Content-Type": code >= 400 ? "application/problem+json" : "application/json" } }));
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", domain: "dev.example", languages: { default: "en", offered: ["en"] } });
      if (url.pathname.endsWith("/permissions/me")) return json(GRANTS);
      if (url.pathname.startsWith("/api/endpoint/")) return json({ jsonrpc: "2.0", id: 1, result: { tools: [] } });
      if (request.method !== "GET") {
        writes.push({ method: request.method, path: url.pathname + url.search });
        if (url.searchParams.get("dryRun") === "All") return json({ valid: true, verdict: { ok: true, findings: [] } });
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-mcp-9", namespace: "helsinki" }, spec: { lane: "Yellow" }, status: { phase: "Proposed" } }, 202);
      }
      if (url.pathname === "/api/v1/projects") {
        if (projects.down) return json({ title: "Unavailable", status: 503, detail: "the project list is down" }, 503);
        return json(list([{ name: "helsinki" }]));
      }
      if (url.pathname === "/api/v1/projects/helsinki/endpoints") return json(list([BIKES]));
      if (url.pathname === "/api/v1/projects/helsinki/mcpservers") return json(list(options.servers ?? [SERVER]));
      if (url.pathname === "/api/v1/projects/helsinki/knowledge/sources") return json({ items: [] });
      return json(list([]));
    }),
  );
  window.history.pushState({}, "", "/projects/helsinki/mcp");
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { writes, projects };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("MCP servers: removing and closing", () => {
  it("asks before removing a server, and backing out removes nothing", async () => {
    const user = userEvent.setup();
    const { writes } = renderAt();
    await user.click(await screen.findByRole("button", { name: "Remove mobility" }));
    const asking = await screen.findByRole("dialog", { name: en.resourceDelete.title.replace("{{name}}", "mobility").replace("{name}", "mobility") });
    await user.click(within(asking).getByRole("button", { name: en.form.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // The only request is the dry run that lists what would go with it; nothing is removed.
    expect(writes.every((w) => w.method === "DELETE" && w.path.includes("dryRun"))).toBe(true);
    expect(screen.getByRole("region", { name: "Mobility data" })).toBeInTheDocument();
  });

  it("closes the form with its Close button once the Change is proposed", async () => {
    const user = userEvent.setup();
    const { writes } = renderAt({ servers: [] });
    await user.click(await screen.findByRole("button", { name: en.mcp.new }));
    const dialog = await screen.findByRole("dialog", { name: en.mcp.form.newTitle });
    await user.type(within(dialog).getByLabelText(/^Name/), "mobility");
    await user.click(await within(dialog).findByRole("checkbox", { name: "bikes (helsinki/bikes)" }));
    await user.click(within(dialog).getByRole("button", { name: en.mcp.form.propose }));
    expect(await within(dialog).findByText("chg-mcp-9", { exact: false })).toBeInTheDocument();
    expect(writes.map((w) => w.method)).toEqual(["POST", "POST"]);
    const closers = within(dialog).getAllByRole("button", { name: en.app.close });
    await user.click(closers.find((button) => button.textContent === en.app.close) ?? closers[0]);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: en.mcp.form.newTitle })).toBeNull());
  });

  it("says the project list could not be read and reads it again on Try again", async () => {
    const user = userEvent.setup();
    const { projects } = renderAt({ servers: [] });
    const open = await screen.findByRole("button", { name: en.mcp.new });
    // The list read at page load answered; the read the form makes when it opens does not.
    projects.down = true;
    await user.click(open);
    const dialog = await screen.findByRole("dialog", { name: en.mcp.form.newTitle });
    await user.click(await within(dialog).findByRole("checkbox", { name: "bikes (helsinki/bikes)" }));
    await user.selectOptions(within(dialog).getByLabelText(/^Who may connect/), "project-list");
    await user.type(within(dialog).getByLabelText(/^Projects it admits/), "h");
    expect(await screen.findByText(/The list could not be loaded, try again/)).toBeInTheDocument();
    projects.down = false;
    await user.click(screen.getByRole("button", { name: en.form.listRetry }));
    expect(await screen.findByRole("option", { name: /helsinki/ })).toBeInTheDocument();
  });
});

describe("copying a value", () => {
  it("keeps saying Copy when the clipboard refuses", async () => {
    const writeText = vi.fn(() => Promise.reject(new Error("denied")));
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(
      <I18nextProvider i18n={i18n}>
        <CopyValue label="Address" value="https://api.dev.example/x" />
      </I18nextProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Copy: Address" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("https://api.dev.example/x"));
    expect(screen.getByRole("button", { name: "Copy: Address" })).toHaveTextContent(en.mcp.copy);
    expect(screen.queryByText(en.mcp.copied)).toBeNull();
  });
});
