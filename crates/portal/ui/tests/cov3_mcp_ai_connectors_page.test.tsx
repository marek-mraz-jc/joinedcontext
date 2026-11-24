// T-7589: AiConnectorsPage: the card's setup toggle, tab pairing, activity, edit; the form's
// presets, verbs, autonomous tick, date, a refusal naming a kind, closing with Escape; toForm for
// a connector made before verbs were listed.
import { act } from "react";
import type { JSX } from "react";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { hydrateRoot } from "react-dom/client";
import { renderToString } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AiConnectorCard, AiConnectorsPage } from "../src/pages/mcp/AiConnectorsPage";
import { toForm } from "../src/pages/mcp/aiConnectors";
import type { AiConnector, ConnectorOptions } from "../src/pages/mcp/aiConnectors";

const listeners = new Set<() => void>();
const paired = vi.hoisted(() => ({ connector: null as string | null }));
vi.mock("../src/connectors/pairedTab", () => ({
  connectTab: vi.fn(async (id: string) => {
    paired.connector = id;
    listeners.forEach((listener) => listener());
    return undefined;
  }),
  disconnectTab: vi.fn(async () => {
    paired.connector = null;
    listeners.forEach((listener) => listener());
  }),
  onPairingChange: vi.fn((listener: () => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }),
  pairing: vi.fn(() => (paired.connector ? { connector: paired.connector } : null)),
}));

const OPTIONS: ConnectorOptions = {
  segments: ["data-read", "context", "pipeline"],
  segmentKinds: [
    { segment: "context", kinds: ["Endpoint", "Policy"] },
    { segment: "pipeline", kinds: ["Pipeline"] },
  ],
  ceiling: {
    segments: ["data-read", "context", "pipeline"],
    writes: true,
    maxDays: 30,
    kinds: [
      { kind: "Endpoint", verbs: ["read", "create", "edit"] },
      { kind: "Policy", verbs: ["read"] },
      { kind: "Pipeline", verbs: ["read", "run"] },
    ],
  },
  projects: [{ project: "doprava", spaces: ["parking"], endpoints: ["bike-counters"], pipelines: ["load"], allowed: true }],
  clients: [{ id: "mcp-local", isBoundary: false }],
};
const URL_OF = "https://portal.dev.example/c/abcdefghijklmnopqrstuvwxyz";
const connector = (patch: Partial<AiConnector> = {}): AiConnector => ({
  id: "abcdefghijklmnopqrstuvwxyz",
  name: "doprava-in-claude",
  owner: "b7c1e0f4",
  segments: ["context"],
  scope: [{ project: "doprava", endpoint: "bike-counters", access: "read" }],
  writes: false,
  autonomous: false,
  expiresAt: new Date(Date.now() + 10 * 24 * 3600_000).toISOString(),
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  limits: { parallelCalls: 4, resultBytes: 1048576 },
  state: "active",
  createdAt: "2026-10-28T12:00:00Z",
  lastUsedAt: null,
  url: URL_OF,
  ...patch,
});

interface Stub {
  items?: AiConnector[];
  activity?: "fails-once" | "empty";
  refuse?: { status: number; detail: string; errors?: string[] };
}
type Seen = { method: string; path: string; body?: unknown };

function renderPage(stub: Stub = {}, page: JSX.Element = <AiConnectorsPage />): Seen[] {
  const seen: Seen[] = [];
  let activityReads = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, code = 200) =>
        Promise.resolve(new Response(code === 204 ? null : JSON.stringify(body), { status: code, headers: { "Content-Type": code >= 400 ? "application/problem+json" : "application/json" } }));
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "b7c1e0f4", username: "eva", name: "Eva", roles: [] });
      if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", domain: "dev.example", languages: { default: "en", offered: ["en"] } });
      if (url.pathname.endsWith("/permissions/me")) return json({ project: "doprava", bootstrap: false, grants: [] });
      if (url.pathname.startsWith("/api/v1/connectors")) {
        const body = request.method === "GET" ? undefined : await request.json();
        seen.push({ method: request.method, path: url.pathname, body });
        if (url.pathname === "/api/v1/connectors/options") return json(OPTIONS);
        if (url.pathname.endsWith("/activity")) {
          activityReads += 1;
          if (stub.activity === "fails-once" && activityReads === 1) return json({ title: "Unavailable", status: 503, detail: "the log is not answering" }, 503);
          return json({ items: [] });
        }
        if (url.pathname === "/api/v1/connectors" && request.method === "GET") return json({ items: stub.items ?? [] });
        if (request.method === "POST" && stub.refuse) return json({ title: "Refused", ...stub.refuse }, stub.refuse.status);
        if (request.method === "POST") return json(connector({ name: (body as { name: string }).name }), 201);
        if (request.method === "PATCH") return json({ ...connector(), ...(body as object) });
      }
      return json({ items: [] });
    }),
  );
  window.history.pushState({}, "", "/ai-connectors");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{page}</I18nextProvider>
    </QueryClientProvider>,
  );
  return seen;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  paired.connector = null;
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("an AI connector card", () => {
  it("lists the changes it may propose in words, and opens and folds its setup lines", async () => {
    const user = userEvent.setup();
    renderPage({ items: [connector({ kinds: [{ kind: "Endpoint", verbs: ["read", "create", "edit"] }, { kind: "Policy", verbs: ["read"] }] })] });
    expect(await screen.findByText("Endpoint: create, edit")).toBeInTheDocument();
    expect(screen.queryByText(/Policy: /)).toBeNull();
    const setup = screen.getByRole("button", { name: en.aiConnectors.setup });
    await user.click(setup);
    expect(screen.getByText(en.aiConnectors.setupTitle)).toBeInTheDocument();
    expect(setup).toHaveAttribute("aria-expanded", "true");
    await user.click(setup);
    expect(screen.queryByText(en.aiConnectors.setupTitle)).toBeNull();
  });

  it("connects this tab to the connector and disconnects it again", async () => {
    const user = userEvent.setup();
    renderPage({ items: [connector()] });
    const connect = await screen.findByRole("button", { name: `Connect this tab to doprava-in-claude` });
    await user.click(connect);
    const disconnect = await screen.findByRole("button", { name: "Disconnect this tab from doprava-in-claude" });
    expect(disconnect).toHaveAttribute("aria-pressed", "true");
    await user.click(disconnect);
    expect(await screen.findByRole("button", { name: "Connect this tab to doprava-in-claude" })).toBeInTheDocument();
  });

  it("renders the same card when the page was drawn on a server first", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(OPTIONS), { headers: { "Content-Type": "application/json" } })));
    const tree = (
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AiConnectorCard connector={connector()} />
        </I18nextProvider>
      </QueryClientProvider>
    );
    const host = document.createElement("div");
    host.innerHTML = renderToString(tree);
    document.body.append(host);
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    await act(async () => {
      hydrateRoot(host, tree);
    });
    errors.mockRestore();
    expect(await within(host).findByRole("region", { name: "doprava-in-claude" })).toBeInTheDocument();
    host.remove();
  });

  it("says the activity could not be read, reads it again on retry and then says no calls are kept", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ items: [connector()], activity: "fails-once" });
    await user.click(await screen.findByRole("button", { name: en.aiConnectors.activity }));
    expect(await screen.findByText(/the log is not answering/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.aiConnectors.noCalls)).toBeInTheDocument();
    expect(seen.filter((one) => one.path.endsWith("/activity"))).toHaveLength(2);
  });

  it("edits a connector, saves it with PATCH and closes the form; cancelling closes it too", async () => {
    const user = userEvent.setup();
    const seen = renderPage({ items: [connector()] });
    const edit = await screen.findByRole("button", { name: "Edit AI connector doprava-in-claude" });
    await user.click(edit);
    let dialog = await screen.findByRole("dialog", { name: "Edit doprava-in-claude" });
    await user.click(within(dialog).getByRole("button", { name: en.form.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(edit);
    dialog = await screen.findByRole("dialog", { name: "Edit doprava-in-claude" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: en.aiConnectors.save })).toBeEnabled());
    await user.click(within(dialog).getByRole("button", { name: en.aiConnectors.save }));
    await waitFor(() => expect(seen.some((one) => one.method === "PATCH")).toBe(true));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("the new AI connector form", () => {
  it("proposes verbs under a preset, writes with autonomy and a chosen day, and closes on Escape", async () => {
    const user = userEvent.setup();
    const seen = renderPage();
    await user.click((await screen.findAllByRole("button", { name: en.aiConnectors.new }))[0]);
    let dialog = await screen.findByRole("dialog", { name: en.aiConnectors.newTitle });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click((await screen.findAllByRole("button", { name: en.aiConnectors.new }))[0]);
    dialog = await screen.findByRole("dialog", { name: en.aiConnectors.newTitle });
    await user.type(within(dialog).getByRole("textbox", { name: /Name/ }), "writer");
    await user.click(await within(dialog).findByRole("checkbox", { name: /^Context \(models/ }));
    const endpointVerbs = within(dialog).getByRole("group", { name: "What it may do on Endpoints" });
    await user.click(within(endpointVerbs).getByRole("checkbox", { name: en.aiConnectors.verb.create }));
    expect(within(endpointVerbs).getByRole("checkbox", { name: en.aiConnectors.verb.create })).toBeChecked();
    await user.click(within(endpointVerbs).getByRole("checkbox", { name: en.aiConnectors.verb.create }));
    await user.click(within(endpointVerbs).getByRole("checkbox", { name: en.aiConnectors.verb.edit }));
    await user.click(within(dialog).getByRole("checkbox", { name: "bike-counters" }));
    const autonomous = within(dialog).getByRole("checkbox", { name: /^Approve its changes at once/ });
    expect(autonomous).toHaveAttribute("aria-disabled", "true");
    await user.selectOptions(within(dialog).getByRole("combobox", { name: "Access for bike-counters" }), "write");
    await user.click(autonomous);
    expect(autonomous).toBeChecked();
    const day = within(dialog).getByLabelText(new RegExp(`^${en.aiConnectors.field.expires}`));
    fireEvent.change(day, { target: { value: new Date(Date.now() + 5 * 24 * 3600_000).toISOString().slice(0, 10) } });
    await user.click(within(dialog).getByRole("button", { name: en.aiConnectors.create }));
    await waitFor(() => expect(seen.some((one) => one.method === "POST")).toBe(true));
    const sent = seen.find((one) => one.method === "POST")?.body as { segments: string[]; autonomous: boolean; writes: boolean; expiresAt: string; kinds: { kind: string; verbs: string[] }[] };
    expect(sent.segments).toEqual(expect.arrayContaining(["data-read", "context"]));
    expect([sent.autonomous, sent.writes]).toEqual([true, true]);
    expect(sent.expiresAt).toBe(`${new Date(Date.now() + 5 * 24 * 3600_000).toISOString().slice(0, 10)}T23:59:59Z`);
    expect(sent.kinds).toContainEqual({ kind: "Endpoint", verbs: ["read", "edit"] });
  });

  it("puts a refusal of a kind's verbs beside that kind", async () => {
    const user = userEvent.setup();
    renderPage({ refuse: { status: 422, detail: "kinds[0].verbs: your organization lets Endpoint be proposed by approvers only", errors: ["kinds[0].verbs"] } });
    await user.click((await screen.findAllByRole("button", { name: en.aiConnectors.new }))[0]);
    const dialog = await screen.findByRole("dialog", { name: en.aiConnectors.newTitle });
    await user.type(within(dialog).getByRole("textbox", { name: /Name/ }), "refused");
    await user.click(await within(dialog).findByRole("checkbox", { name: /^Context \(models/ }));
    await user.click(within(dialog).getByRole("checkbox", { name: "bike-counters" }));
    await user.click(within(dialog).getByRole("button", { name: en.aiConnectors.create }));
    expect(await within(dialog).findByText("your organization lets Endpoint be proposed by approvers only")).toBeInTheDocument();
  });
});

describe("a connector made before its verbs were listed", () => {
  it("shows every verb the ceiling lets it tick on the kinds its presets reach", () => {
    const form = toForm(connector({ segments: ["context", "pipeline"], kinds: undefined }), OPTIONS);
    expect(form.verbs).toEqual({ Endpoint: ["create", "edit"], Policy: [], Pipeline: ["run"] });
    expect(toForm(connector({ kinds: undefined })).verbs).toEqual({});
  });
});
