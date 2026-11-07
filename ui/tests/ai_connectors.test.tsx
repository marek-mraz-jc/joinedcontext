// covers (the module gate in gate_modules.test.ts): src/pages/mcp/aiConnectors.ts and
// src/pages/mcp/AiConnectorsPage.tsx, a person's AI connectors at /c/{id} (AG-142, T-5951).
import { useState } from "react";
import type { JSX } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { narrowsAndOrders } from "./grid_checks";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { AiConnectorsPage, ClientsField, KindVerbsPicker, NothingToReach, PresetsField, ScopeTree, SetupPanel } from "../src/pages/mcp/AiConnectorsPage";
import { clientLabel, clientRefusal, emptyForm, formProblems, kindRows, namedClients, readsShared, refusedSection, scopeLines, setup, toForm, toRequest } from "../src/pages/mcp/aiConnectors";
import type { AiConnector, ConnectorOptions } from "../src/pages/mcp/aiConnectors";

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", roles: [] };
const NOW = new Date("2026-10-28T12:00:00Z");
const OPTIONS: ConnectorOptions = {
  segments: ["data-read", "context", "pipeline", "access", "publish", "apps", "operate", "knowledge", "admin"],
  segmentKinds: [
    { segment: "context", kinds: ["ContextSpace", "DataModel", "Endpoint", "Policy"] },
    { segment: "pipeline", kinds: ["Pipeline", "DataSource", "Endpoint"] },
    { segment: "apps", kinds: ["App", "Dashboard", "Layer"] },
  ],
  ceiling: {
    segments: ["data-read"],
    writes: false,
    maxDays: 30,
    kinds: [
      { kind: "Endpoint", verbs: ["read"] },
      { kind: "ContextSpace", verbs: ["read"] },
      { kind: "DataModel", verbs: ["read"] },
    ],
  },
  projects: [{ project: "doprava", spaces: ["parking"], endpoints: ["bike-counters"], pipelines: ["load-counters"], allowed: true }],
  clients: [
    { id: "mcp-local", isBoundary: false },
    { id: "https://claude.ai/oauth/mcp-client-metadata", isBoundary: true },
  ],
};
const URL_OF = "https://portal.dev.example/c/abcdefghijklmnopqrstuvwxyz";
const connector = (patch: Partial<AiConnector> = {}): AiConnector => ({
  id: "abcdefghijklmnopqrstuvwxyz",
  name: "doprava-in-claude",
  owner: IDENTITY.subject,
  segments: ["data-read"],
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

describe("AI connector helpers", () => {
  it("turns ticks into scope items, writes only when an item writes, and ends at the day's end", () => {
    const form = { ...emptyForm(OPTIONS, NOW), name: "a", picks: [
      { project: "doprava", kind: "project" as const, access: "read" as const },
      { project: "doprava", kind: "endpoint" as const, name: "bike-counters", access: "read" as const },
    ] };
    expect(form.segments).toEqual(["data-read"]);
    expect(form.expires).toBe("2026-11-26");
    expect(toRequest(form)).toEqual({
      name: "a",
      segments: ["data-read"],
      scope: [{ project: "doprava", access: "read" }, { project: "doprava", endpoint: "bike-counters", access: "read" }],
      kinds: [],
      writes: false,
      autonomous: false,
      expiresAt: "2026-11-26T23:59:59Z",
      allowedClients: [],
    });
    expect(toRequest({ ...form, picks: [{ project: "doprava", kind: "space", name: "parking", access: "write" }] }).writes).toBe(true);
    // T-6552: autonomous goes out only with a write pick, and comes back from the stored connector.
    expect(toRequest({ ...form, autonomous: true }).autonomous).toBe(false);
    expect(toRequest({ ...form, autonomous: true, picks: [{ project: "doprava", kind: "space", name: "parking", access: "write" }] }).autonomous).toBe(true);
    expect(toForm(connector({ autonomous: true })).autonomous).toBe(true);
    expect(refusedSection(["autonomous"], "autonomous: no")).toBe("autonomous");
  });

  it("sends read alone on every kind the presets reach unless a verb is ticked (T-6302)", () => {
    const form = { ...emptyForm(OPTIONS, NOW), segments: ["context", "apps"], verbs: { Endpoint: ["create" as const], App: ["edit" as const] } };
    expect(toRequest(form, OPTIONS).kinds).toEqual([
      { kind: "ContextSpace", verbs: ["read"] },
      { kind: "DataModel", verbs: ["read"] },
      { kind: "Endpoint", verbs: ["read", "create"] },
      { kind: "Policy", verbs: ["read"] },
      { kind: "App", verbs: ["read", "edit"] },
      { kind: "Dashboard", verbs: ["read"] },
      { kind: "Layer", verbs: ["read"] },
    ]);
    expect(kindRows(OPTIONS, ["data-read", "context", "pipeline"]).map((row) => row.kinds)).toEqual([["ContextSpace", "DataModel", "Endpoint", "Policy"], ["Pipeline", "DataSource"]]);
    expect(toForm(connector({ kinds: [{ kind: "Endpoint", verbs: ["read", "propose"] }] })).verbs).toEqual({ Endpoint: ["create", "edit", "delete"] });
    expect(refusedSection(["kinds[0].verbs"], "kinds[0].verbs: no")).toBe("kinds");
  });

  it("refuses a bad name, no preset, no tick and a day past the cap before the server is asked", () => {
    const problems = formProblems({ name: "Bad Name", segments: [], picks: [], verbs: {}, expires: "2027-01-01", clients: [], autonomous: false }, OPTIONS, NOW);
    expect(Object.keys(problems).sort()).toEqual(["expiresAt", "name", "scope", "segments"]);
    expect(formProblems({ ...emptyForm(OPTIONS, NOW), name: "ok", picks: [{ project: "doprava", kind: "project", access: "read" }] }, OPTIONS, NOW)).toEqual({});
  });

  it("names clients from a fixed map, never from the id's own text (T-6309)", () => {
    const t = i18n.getFixedT("en");
    expect(clientLabel("https://claude.ai/oauth/mcp-oauth-client-metadata", t)).toBe("claude.ai");
    expect(clientLabel("https://claude.ai/oauth/claude-code-client-metadata", t)).toBe("Claude Code");
    expect(clientLabel("https://claude.ai.evil.example/x", t)).toBe("https://claude.ai.evil.example/x");
    expect(clientLabel("toString", t)).toBe("toString");
    const clients = [{ id: "https://claude.ai/oauth/claude-code-client-metadata" }, { id: "mcp-hub" }];
    expect(namedClients("allowedClients[0]: p does not allow the client https://claude.ai/oauth/claude-code-client-metadata", clients, t)).toBe(
      "p does not allow the client Claude Code",
    );
  });

  it("says a clients' refusal in plain words: whom it names and whom the organization allows (T-6424)", () => {
    const t = i18n.getFixedT("en");
    const code = "https://claude.ai/oauth/claude-code-client-metadata";
    const ai = "https://claude.ai/oauth/mcp-oauth-client-metadata";
    const clients = [{ id: "mcp-hub", refused: "no" }, { id: code }, { id: ai }];
    const said = clientRefusal(`allowedClients[0]: DelegationPolicy claude-only does not allow the client ${code}`, clients, t);
    expect(said).toEqual({ id: code, text: "Your organization allows only these clients: claude.ai. Untick Claude Code." });
    expect(said.text).not.toContain("DelegationPolicy");
    expect(clientRefusal(`p does not allow the client ${code}`, [{ id: code }], t).text).toMatch(/allows no client for this connector yet\. Untick Claude Code/);
    // A client the options never offered, or none at all: the server's text, ids in words.
    expect(clientRefusal("p does not allow the client of this token (none named)", clients, t)).toEqual({ text: "p does not allow the client of this token (none named)" });
    expect(clientRefusal(`allowedClients: ${code} is not a client`, clients, t)).toEqual({ text: "Claude Code is not a client" });
  });

  it("reads a connector as organization data only when every item sits in a shared project (T-6424)", () => {
    const options = { ...OPTIONS, projects: [{ project: "helsinki", spaces: [], endpoints: ["a"], pipelines: [], allowed: true, organizationData: true }, ...OPTIONS.projects] };
    const item = (project: string) => ({ project, endpoint: "a", access: "read" as const });
    expect(readsShared(connector({ scope: [item("helsinki")] }), options)).toBe(true);
    expect(readsShared(connector({ scope: [item("helsinki"), item("doprava")] }), options)).toBe(false);
    expect(readsShared(connector({ scope: [] }), options)).toBe(false);
    expect(readsShared(connector({ scope: [item("helsinki")] }), undefined)).toBe(false);
  });

  it("finds the section a refusal names, from errors or a 403's detail", () => {
    expect(refusedSection(["scope[0].endpoint"], "x")).toBe("scope");
    expect(refusedSection(undefined, "segments[1]: your organization lets a connector carry only data-read (AG-150)")).toBe("segments");
    expect(refusedSection(undefined, "writes: your organization lets a connector read only")).toBe("scope");
    expect(refusedSection(undefined, "expiresAt: a connector lives at most 30 days (AG-150)")).toBe("expiresAt");
    expect(refusedSection(undefined, "a live connector of that name exists")).toBeUndefined();
  });

  it("summarizes scope and writes the setup lines with the connector's own address", () => {
    expect(scopeLines([{ project: "doprava", access: "read" }, { project: "doprava", pipeline: "*", access: "read" }])).toEqual([
      { where: "doprava", kind: "project", name: undefined, access: "read" },
      { where: "doprava", kind: "pipeline", name: "*", access: "read" },
    ]);
    const lines = setup("doprava-in-claude", URL_OF);
    expect(lines.claudeCode).toBe(`claude mcp add --transport http doprava-in-claude ${URL_OF}`);
    expect(JSON.parse(lines.vscode)).toEqual({ servers: { "doprava-in-claude": { type: "http", url: URL_OF } } });
  });
});

interface Stub {
  items?: AiConnector[];
  refuse?: { status: number; detail: string; errors?: string[] };
  listFails?: boolean;
  activity?: unknown[];
  options?: ConnectorOptions;
}

const MEMBER: ConnectorOptions = {
  ...OPTIONS,
  ceiling: { ...OPTIONS.ceiling, segments: ["data-read"], writes: false, kinds: [] },
  projects: [{ project: "helsinki", spaces: [], endpoints: ["traffic-counts"], pipelines: [], allowed: true, organizationData: true }],
};

function renderAt(path: string, stub: Stub = {}, page?: JSX.Element) {
  const requests: { method: string; path: string; body?: unknown }[] = [];
  let items = stub.items ?? [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, code = 200) =>
        Promise.resolve(new Response(code === 204 ? null : JSON.stringify(body), { status: code, headers: { "Content-Type": code >= 400 ? "application/problem+json" : "application/json" } }));
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", domain: "dev.example", languages: { default: "en", offered: ["en"] } });
      if (url.pathname.endsWith("/permissions/me")) return json({ project: "doprava", bootstrap: false, grants: [] });
      if (url.pathname === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "doprava" }] });
      if (url.pathname.startsWith("/api/v1/connectors")) {
        const body = request.method === "POST" || request.method === "PATCH" ? await request.json() : undefined;
        requests.push({ method: request.method, path: url.pathname, body });
        if (url.pathname === "/api/v1/connectors/options") return json(stub.options ?? OPTIONS);
        if (url.pathname === "/api/v1/connectors" && request.method === "GET") {
          return stub.listFails ? json({ title: "Server error", status: 500, detail: "the store did not answer" }, 500) : json({ items });
        }
        if (url.pathname === "/api/v1/connectors" && request.method === "POST") {
          if (stub.refuse) return json({ title: "Refused", status: stub.refuse.status, detail: stub.refuse.detail, ...(stub.refuse.errors ? { errors: stub.refuse.errors } : {}) }, stub.refuse.status);
          const made = connector({ name: (body as { name: string }).name });
          items = [made];
          return json(made, 201);
        }
        if (url.pathname.endsWith("/activity")) {
          if (stub.activity) return json({ items: stub.activity });
          return json({ items: [{ at: "2026-10-28T12:30:00Z", subject: "b7c1e0f4", client: "mcp-local", tool: "jc_resource_get", target: "doprava/Endpoint/bike-counters", verdict: "allowed", durationMs: 12 }] });
        }
        if (request.method === "PATCH") {
          const patch = body as Partial<AiConnector>;
          items = items.map((one) => ({ ...one, ...patch }));
          return json(items[0]);
        }
        if (request.method === "DELETE") {
          items = items.map((one) => ({ ...one, state: "revoked" }));
          return json(null, 204);
        }
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{page ?? <App />}</I18nextProvider>
    </QueryClientProvider>,
  );
  return { requests };
}

describe("the AI connectors page (T-5951)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says what to do first when the person holds none", async () => {
    renderAt("/ai-connectors", {}, <AiConnectorsPage />);
    expect(await screen.findByText("No AI connectors yet")).toBeInTheDocument();
    expect(screen.getByText(/paste its address into Claude Code/)).toBeInTheDocument();
  });

  // Older revoked cards pushed the live ones and the new one down the page (journey clip, T-6454).
  it("folds what was revoked before this visit under one line", async () => {
    const items = [connector({ id: "a".repeat(26), name: "live-one" }), connector({ id: "b".repeat(26), name: "old-one", state: "revoked" })];
    renderAt("/ai-connectors", { items }, <AiConnectorsPage />);
    await screen.findByRole("region", { name: "live-one" });
    const folded = screen.getByText("Revoked (1)").closest("details");
    expect(folded).not.toHaveAttribute("open");
    expect(folded).toContainElement(screen.getByRole("region", { name: "old-one", hidden: true }));
  });

  it("shows a failed list with a retry", async () => {
    renderAt("/ai-connectors", { listFails: true }, <AiConnectorsPage />);
    expect(await screen.findByRole("button", { name: /Retry/ })).toBeInTheDocument();
  });

  it("creates from the ticks, shows the setup lines and no token, then revokes after an inline confirm", async () => {
    const user = userEvent.setup();
    const { requests } = renderAt("/ai-connectors", {}, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByRole("textbox", { name: /Name/ }), "doprava-in-claude");
    await user.click(within(dialog).getByRole("checkbox", { name: "bike-counters" }));
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(requests.some((one) => one.method === "POST")).toBe(true));
    const sent = requests.find((one) => one.method === "POST")?.body as { scope: unknown; segments: string[]; writes: boolean };
    expect(sent.scope).toEqual([{ project: "doprava", endpoint: "bike-counters", access: "read" }]);
    expect([sent.segments, sent.writes]).toEqual([["data-read"], false]);
    expect((sent as unknown as { kinds: unknown }).kinds).toEqual([]);

    expect(await screen.findByText(`claude mcp add --transport http doprava-in-claude ${URL_OF}`)).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/eyJ|Bearer|token=/);

    await user.click(screen.getByRole("button", { name: "Revoke AI connector doprava-in-claude" }));
    expect(screen.getByRole("alert")).toHaveTextContent("The next call through it is refused");
    await user.click(screen.getByRole("button", { name: "Revoke now" }));
    await waitFor(() => expect(requests.some((one) => one.method === "DELETE")).toBe(true));
    expect(await screen.findByText("Revoked")).toBeInTheDocument();
  });

  it.each([
    ["pending", "Waiting for approval"],
    ["expired", "Expired"],
  ])("names a connector in state %s and lets its owner revoke it (JC2-25)", async (state, word) => {
    renderAt("/ai-connectors", { items: [connector({ state })] }, <AiConnectorsPage />);
    expect(await screen.findByRole("button", { name: "Revoke AI connector doprava-in-claude" })).toBeInTheDocument();
    expect(screen.getByText(word)).toBeInTheDocument();
  });

  it("offers the clients the ceiling lists by name and greys a realm client it refuses (T-6309)", async () => {
    const user = userEvent.setup();
    const refused = "DelegationPolicy claude-only does not allow the client mcp-hub";
    const options: ConnectorOptions = {
      ...OPTIONS,
      clients: [
        { id: "mcp-hub", isBoundary: false, refused },
        { id: "https://claude.ai/oauth/claude-code-client-metadata", isBoundary: false },
        { id: "https://claude.ai/oauth/mcp-oauth-client-metadata", isBoundary: true },
        { id: "https://other.example/client.json", isBoundary: false },
      ],
    };
    const { requests } = renderAt("/ai-connectors", { options }, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    const hub = within(dialog).getByRole("checkbox", { name: /The organization's MCP hub/ });
    expect(hub).toHaveAttribute("aria-disabled", "true");
    expect(within(dialog).queryByText(refused)).not.toBeInTheDocument();
    expect(within(dialog).getByText("Your organization allows only the clients its policy lists.")).toBeInTheDocument();
    expect(within(dialog).getByText(/Tick the client you will use/)).toBeInTheDocument();
    await user.click(hub);
    expect(hub).not.toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: /^Claude Code/ })).not.toHaveAttribute("aria-disabled");
    expect(within(dialog).getByRole("checkbox", { name: /https:\/\/other\.example\/client\.json/ })).toBeInTheDocument();
    await user.type(within(dialog).getByRole("textbox", { name: /Name/ }), "air-in-claude");
    await user.click(within(dialog).getByRole("checkbox", { name: "bike-counters" }));
    await user.click(within(dialog).getByRole("checkbox", { name: /^claude\.ai/ }));
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(requests.some((one) => one.method === "POST")).toBe(true));
    const sent = requests.find((one) => one.method === "POST")?.body as { allowedClients: string[] };
    expect(sent.allowedClients).toEqual(["https://claude.ai/oauth/mcp-oauth-client-metadata"]);
  });

  it("puts a clients' refusal under the client it names, marked, until that client is unticked (T-6424)", async () => {
    const user = userEvent.setup();
    const code = "https://claude.ai/oauth/claude-code-client-metadata";
    const ai = "https://claude.ai/oauth/mcp-oauth-client-metadata";
    const options: ConnectorOptions = { ...OPTIONS, clients: [{ id: "mcp-hub", isBoundary: false, refused: "no" }, { id: code, isBoundary: false }, { id: ai, isBoundary: true }] };
    const Field = () => {
      const [clients, setClients] = useState([code]);
      return <ClientsField options={options} form={{ ...emptyForm(options, NOW), clients }} refused={`allowedClients[0]: DelegationPolicy claude-only does not allow the client ${code}`} onChange={setClients} />;
    };
    render(
      <I18nextProvider i18n={i18n}>
        <Field />
      </I18nextProvider>,
    );
    const box = screen.getByRole("checkbox", { name: /^Claude Code\s*not allowed/ });
    expect(box).toHaveAttribute("aria-invalid", "true");
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Your organization allows only these clients: claude.ai. Untick Claude Code.");
    expect(box.compareDocumentPosition(alert) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(alert.compareDocumentPosition(screen.getByRole("checkbox", { name: /^claude\.ai/ })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await user.click(box);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /^Claude Code/ })).not.toHaveAttribute("aria-invalid");
  });

  it("holds Create while a client the server refused stays ticked, with the reason (T-6424)", async () => {
    const user = userEvent.setup();
    const code = "https://claude.ai/oauth/claude-code-client-metadata";
    const options: ConnectorOptions = { ...OPTIONS, clients: [{ id: code, isBoundary: false }, { id: "https://claude.ai/oauth/mcp-oauth-client-metadata", isBoundary: true }] };
    const detail = `allowedClients[0]: DelegationPolicy claude-only does not allow the client ${code}`;
    renderAt("/ai-connectors", { options, refuse: { status: 403, detail, errors: ["allowedClients[0]"] } }, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    await user.type(await within(dialog).findByRole("textbox", { name: /Name/ }), "air-in-claude");
    await user.click(within(dialog).getByRole("checkbox", { name: "bike-counters" }));
    await user.click(within(dialog).getByRole("checkbox", { name: /^Claude Code/ }));
    const create = within(dialog).getByRole("button", { name: "Create" });
    await user.click(create);
    await within(dialog).findByRole("alert");
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).toHaveAccessibleDescription(/allows only these clients: claude\.ai/);
    await user.click(within(dialog).getByRole("checkbox", { name: /^Claude Code/ }));
    expect(create).not.toHaveAttribute("aria-disabled");
  });

  it("marks a ticked client the ceiling refuses before Create is pressed (T-6424)", () => {
    const options: ConnectorOptions = { ...OPTIONS, clients: [{ id: "mcp-hub", isBoundary: false, refused: "no" }] };
    render(
      <I18nextProvider i18n={i18n}>
        <ClientsField options={options} form={{ ...emptyForm(options, NOW), clients: ["mcp-hub"] }} onChange={() => undefined} />
      </I18nextProvider>,
    );
    const box = screen.getByRole("checkbox", { name: /not allowed/ });
    expect(box).not.toHaveAttribute("aria-disabled");
    expect(box).toHaveAttribute("aria-invalid", "true");
  });

  it("says once what a tick means when every client's hint would repeat it (T-6424)", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <ClientsField options={OPTIONS} form={emptyForm(OPTIONS, NOW)} onChange={() => undefined} />
      </I18nextProvider>,
    );
    expect(screen.getAllByText(/other local programs can name it too/)).toHaveLength(1);
    expect(screen.getByText(/Leave empty for any client/)).toHaveAttribute("id", "ai-connector-clients-help");
  });

  it("gives a member one offered preset in narrower words and one line for the rest (T-6424)", async () => {
    const user = userEvent.setup();
    renderAt("/ai-connectors", { options: MEMBER }, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("checkbox", { name: /reads the Endpoints shared with you/ })).toBeChecked();
    expect(within(dialog).queryByText(/Endpoints, spaces and models/)).not.toBeInTheDocument();
    expect(within(dialog).queryByText(/needs your organization's approval$/)).not.toBeInTheDocument();
    const others = MEMBER.segments.length - 1;
    expect(within(dialog).getByText(`${others} other presets need your organization's approval.`)).toBeInTheDocument();
    expect(within(dialog).queryByText(/Greyed presets/)).not.toBeInTheDocument();
  });

  it("keeps showing a preset a member's connector holds above the ceiling, greyed (T-6424)", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <PresetsField options={MEMBER} form={{ ...emptyForm(MEMBER, NOW), segments: ["data-read", "pipeline"] }} readsOnly onChange={() => undefined} />
      </I18nextProvider>,
    );
    const held = screen.getByRole("checkbox", { name: /^Pipelines/ });
    expect(held).toBeChecked();
    expect(held).toHaveAttribute("aria-disabled", "true");
    expect(held).toHaveAccessibleName(/needs your organization's approval/);
    expect(screen.getByText(/^\d+ other presets need/)).toBeInTheDocument();
  });

  it("keeps every preset for a person with a project of their own beside shared data (T-6424)", async () => {
    const user = userEvent.setup();
    const options = { ...OPTIONS, projects: [...MEMBER.projects, ...OPTIONS.projects] };
    renderAt("/ai-connectors", { options }, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/Greyed presets/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/other presets? needs?/)).not.toBeInTheDocument();
  });

  it("disables Create with its reason when nothing can be reached, and shows only what to do next (T-6424)", async () => {
    const user = userEvent.setup();
    const { requests } = renderAt("/ai-connectors", { options: { ...MEMBER, projects: [] } }, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/Ask a project owner/)).toBeInTheDocument();
    const create = within(dialog).getByRole("button", { name: "Create" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).toHaveAccessibleDescription("Nothing to reach yet: ask for access first.");
    expect(within(dialog).queryByRole("textbox", { name: /Name/ })).not.toBeInTheDocument();
    await user.click(create);
    expect(requests.some((one) => one.method === "POST")).toBe(false);
  });

  it("closes the nothing-to-reach dialog from Cancel, and Create stays off (T-6424)", async () => {
    const user = userEvent.setup();
    const closed = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <NothingToReach onClose={closed} />
      </I18nextProvider>,
    );
    expect(screen.getByText(/Ask a project owner/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(closed).toHaveBeenCalledOnce();
  });

  it("names organization data on the card and drops what a paired tab would do (T-6424)", async () => {
    const shared = connector({ name: "shared-in-claude", scope: [{ project: "helsinki", endpoint: "traffic-counts", access: "read" }] });
    renderAt("/ai-connectors", { options: MEMBER, items: [shared] }, <AiConnectorsPage />);
    const scope = await screen.findByRole("list", { name: /shared-in-claude/ });
    await within(scope).findByText(/organization data, read/);
    expect(screen.queryByText(/Connect this tab to let the AI client/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Connect this tab/ })).not.toBeInTheDocument();
  });

  it("shows the server's refusal beside the field it names", async () => {
    const user = userEvent.setup();
    renderAt(
      "/ai-connectors",
      { refuse: { status: 422, detail: "project doprava has no Endpoint bike-counters you may read", errors: ["scope[0].endpoint"] } },
      <AiConnectorsPage />,
    );
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByRole("textbox", { name: /Name/ }), "x");
    await user.click(within(dialog).getByRole("checkbox", { name: "bike-counters" }));
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByText("project doprava has no Endpoint bike-counters you may read")).toBeInTheDocument();
  });

  it("refuses an empty form in place, naming each field", async () => {
    const user = userEvent.setup();
    const { requests } = renderAt("/ai-connectors", {}, <AiConnectorsPage />);
    await user.click((await screen.findAllByRole("button", { name: "New AI connector" }))[0]);
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByRole("checkbox", { name: "bike-counters" });
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(within(dialog).getByText(/Give a name of lowercase letters/)).toBeInTheDocument();
    expect(within(dialog).getByText("Pick at least one project, space, Endpoint or pipeline.")).toBeInTheDocument();
    expect(requests.some((one) => one.method === "POST")).toBe(false);
  });

  it("edits a connector from what it holds, keeping its name", async () => {
    const user = userEvent.setup();
    const { requests } = renderAt("/ai-connectors", { items: [connector()] }, <AiConnectorsPage />);
    await user.click(await screen.findByRole("button", { name: "Edit AI connector doprava-in-claude" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit doprava-in-claude" });
    expect(within(dialog).getByRole("textbox", { name: /Name/ })).toHaveAttribute("readonly");
    expect(await within(dialog).findByRole("checkbox", { name: "bike-counters" })).toBeChecked();
    await user.click(within(dialog).getByRole("checkbox", { name: "doprava: the whole project" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(requests.some((one) => one.method === "PATCH")).toBe(true));
    const sent = requests.find((one) => one.method === "PATCH")?.body as Record<string, unknown>;
    expect(sent.name).toBeUndefined();
    expect(sent.scope).toEqual([
      { project: "doprava", endpoint: "bike-counters", access: "read" },
      { project: "doprava", access: "read" },
    ]);
  });

  it("lists the activity of a connector", async () => {
    const user = userEvent.setup();
    renderAt("/ai-connectors", { items: [connector({ lastUsedAt: "2026-10-28T12:30:00Z" })] }, <AiConnectorsPage />);
    await user.click(await screen.findByRole("button", { name: "Activity" }));
    const table = await screen.findByRole("table", { name: "Calls through doprava-in-claude" });
    expect(within(table).getByText("jc_resource_get")).toBeInTheDocument();
    expect(within(table).getByText("allowed")).toBeInTheDocument();
  });

  // T-6206: the call log is on the shared grid: search, sort by time, filter by verdict.
  it("searches, sorts and filters the activity", async () => {
    const call = (at: string, tool: string, verdict: string) => ({ at, subject: "b7c1e0f4", client: "mcp-local", tool, target: null, verdict, durationMs: 1 });
    const user = userEvent.setup();
    renderAt(
      "/ai-connectors",
      {
        items: [connector()],
        activity: [
          call("2026-10-28T12:30:00Z", "jc_resource_get", "allowed"),
          call("2026-10-28T10:00:00Z", "jc_data_query", "denied"),
          call("2026-10-28T11:00:00Z", "jc_resource_list", "allowed"),
        ],
      },
      <AiConnectorsPage />,
    );
    await user.click(await screen.findByRole("button", { name: "Activity" }));
    const table = await screen.findByRole("table", { name: "Calls through doprava-in-claude" });
    const at = (iso: string) => new Date(iso).toLocaleString("en");
    await narrowsAndOrders(table, {
      search: { label: "Search Calls through doprava-in-claude", text: "jc_data", kept: [at("2026-10-28T10:00:00Z")] },
      sort: {
        column: "When",
        ascending: [at("2026-10-28T10:00:00Z"), at("2026-10-28T11:00:00Z"), at("2026-10-28T12:30:00Z")],
      },
      filter: { value: /^denied/, kept: [at("2026-10-28T10:00:00Z")] },
    });
  });

  it("is reached from the person's menu", async () => {
    renderAt("/ai-connectors");
    expect(await screen.findByRole("heading", { name: "My AI connectors" })).toBeInTheDocument();
  });
});

describe("the scope tree", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("greys write above the ceiling and children of a ticked project", async () => {
    const user = userEvent.setup();
    const picks = vi.fn();
    const form = { ...emptyForm(OPTIONS, NOW), picks: [{ project: "doprava", kind: "project" as const, access: "read" as const }] };
    render(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={OPTIONS} form={form} onChange={picks} />
      </I18nextProvider>,
    );
    const write = screen.getByRole("option", { name: "write (needs approval)" });
    expect(write).toBeDisabled();
    const child = screen.getByRole("checkbox", { name: /bike-counters/ });
    expect(child).toHaveAttribute("aria-disabled", "true");
    await user.click(child);
    expect(picks).not.toHaveBeenCalled();
  });

  // Helsinki's whole list ran to hundreds of ticks with the client and expiry fields below (T-6454).
  it("folds a long project and finds an item across the tree", async () => {
    const user = userEvent.setup();
    const endpoints = Array.from({ length: 13 }, (_, at) => `point-${at}`);
    const options = { ...OPTIONS, projects: [{ project: "doprava", spaces: [], endpoints, pipelines: [], allowed: true }, { project: "ovzdusie", spaces: [], endpoints: ["air-quality"], pipelines: [], allowed: true }] };
    render(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={options} form={emptyForm(options, NOW)} onChange={vi.fn()} />
      </I18nextProvider>,
    );
    expect(screen.getByText("Or pick only some of it (13 items)").closest("details")).not.toHaveAttribute("open");
    await user.type(screen.getByRole("searchbox", { name: /Find a project/ }), "POINT-12");
    expect(screen.getByRole("checkbox", { name: "point-12" })).toBeVisible();
    expect(screen.queryByRole("checkbox", { name: "point-11" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: /air-quality/ })).toBeNull();
  });

  it("keeps a short tree open and without a search", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={OPTIONS} form={emptyForm(OPTIONS, NOW)} onChange={vi.fn()} />
      </I18nextProvider>,
    );
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(screen.getByRole("checkbox", { name: "bike-counters" })).toBeVisible();
  });

  it("greys a project and the objects the ceiling refuses, with the reason (T-6307)", async () => {
    const user = userEvent.setup();
    const picks = vi.fn();
    const options: ConnectorOptions = {
      ...OPTIONS,
      projects: [
        { project: "sharing", spaces: ["bikes"], endpoints: ["bike-share"], pipelines: [], allowed: false },
        { project: "doprava", spaces: ["parking"], endpoints: ["bike-counters", "noise"], pipelines: ["load-counters"], allowed: true, allowedEndpoints: ["bike-counters"] },
      ],
    };
    const form = { ...emptyForm(options, NOW), picks: [{ project: "doprava", kind: "endpoint" as const, name: "noise", access: "read" as const }] };
    render(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={options} form={form} onChange={picks} />
      </I18nextProvider>,
    );
    for (const name of [/sharing: the whole project/, /^bikes/, /bike-share/, /doprava: the whole project/, /^parking/, /load-counters/]) {
      expect(screen.getByRole("checkbox", { name })).toHaveAttribute("aria-disabled", "true");
    }
    expect(screen.getAllByText(/Your organization does not let a connector reach this project yet/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/Your organization lets a connector reach only some Endpoints of this project/).length).toBeGreaterThan(0);
    await user.click(screen.getByRole("checkbox", { name: /sharing: the whole project/ }));
    expect(picks).not.toHaveBeenCalled();
    // The allowed Endpoint ticks; a refused one an edit still holds can be unticked.
    await user.click(screen.getByRole("checkbox", { name: /bike-counters/ }));
    expect(picks).toHaveBeenLastCalledWith(expect.arrayContaining([expect.objectContaining({ name: "bike-counters" })]));
    await user.click(screen.getByRole("checkbox", { name: /^noise/ }));
    expect(picks).toHaveBeenLastCalledWith([]);
  });

  it("offers organization data as its shared Endpoints, read only, never the whole project (T-6413)", async () => {
    const user = userEvent.setup();
    const picks = vi.fn();
    const options: ConnectorOptions = {
      ...OPTIONS,
      projects: [{ project: "helsinki", spaces: [], endpoints: ["traffic-counts"], pipelines: [], allowed: true, organizationData: true }],
    };
    const form = { ...emptyForm(options, NOW), picks: [] };
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={options} form={form} onChange={picks} />
      </I18nextProvider>,
    );
    expect(screen.getByText("helsinki: organization data, read")).toBeInTheDocument();
    expect(screen.getByText(/Only the datasets shared with your organization or a group of yours/)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: /the whole project/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: /traffic-counts/ }));
    expect(picks).toHaveBeenLastCalledWith([{ project: "helsinki", kind: "endpoint", name: "traffic-counts", access: "read" }]);
    const ticked = { ...form, picks: [{ project: "helsinki", kind: "endpoint" as const, name: "traffic-counts", access: "read" as const }] };
    rerender(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={options} form={ticked} onChange={picks} />
      </I18nextProvider>,
    );
    // Read only: no access choice to raise to write.
    expect(screen.queryByRole("combobox", { name: /Access for traffic-counts/ })).not.toBeInTheDocument();
  });

  it("ticks change verbs per kind, read always on, and greys a verb above the ceiling (T-6302)", async () => {
    const user = userEvent.setup();
    const verbs = vi.fn();
    const options = { ...OPTIONS, ceiling: { ...OPTIONS.ceiling, kinds: [{ kind: "Endpoint", verbs: ["read", "create"] }, { kind: "Pipeline", verbs: ["read", "create", "edit", "delete", "run"] }] } } as ConnectorOptions;
    render(
      <I18nextProvider i18n={i18n}>
        <KindVerbsPicker options={options} form={{ ...emptyForm(OPTIONS, NOW), segments: ["pipeline"] }} onChange={verbs} />
      </I18nextProvider>,
    );
    const endpoint = screen.getByRole("group", { name: "What it may do on Endpoints" });
    expect(within(endpoint).getByRole("checkbox", { name: /read/ })).toBeChecked();
    expect(within(endpoint).getByRole("checkbox", { name: /edit/ })).toHaveAttribute("aria-disabled", "true");
    expect(within(endpoint).queryByRole("checkbox", { name: /run/ })).toBeNull();
    await user.click(within(endpoint).getByRole("checkbox", { name: /edit/ }));
    expect(verbs).not.toHaveBeenCalled();
    await user.click(within(endpoint).getByRole("checkbox", { name: /create/ }));
    expect(verbs).toHaveBeenLastCalledWith({ Endpoint: ["create"] });
    const pipeline = screen.getByRole("group", { name: "What it may do on Pipelines" });
    await user.click(within(pipeline).getByRole("checkbox", { name: /run/ }));
    expect(verbs).toHaveBeenLastCalledWith({ Pipeline: ["run"] });
  });

  it("puts a server refusal of the list beside the kind it names", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <KindVerbsPicker options={OPTIONS} form={{ ...emptyForm(OPTIONS, NOW), segments: ["context"] }} onChange={() => undefined} refusal={{ kind: "ContextSpace", message: "no edit on ContextSpace" }} />
      </I18nextProvider>,
    );
    const alert = screen.getByText("no edit on ContextSpace");
    expect(screen.getByRole("group", { name: "What it may do on Context Spaces" }).nextElementSibling).toContainElement(alert);
  });

  it("says what to tick first when no preset is ticked", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <KindVerbsPicker options={OPTIONS} form={{ ...emptyForm(OPTIONS, NOW), segments: [] }} onChange={() => undefined} />
      </I18nextProvider>,
    );
    expect(screen.getByText(/Tick a preset that proposes changes/)).toBeInTheDocument();
  });

  it("says what to do when the person reads no project", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <ScopeTree options={{ ...OPTIONS, projects: [] }} form={emptyForm(OPTIONS, NOW)} onChange={() => undefined} />
      </I18nextProvider>,
    );
    expect(screen.getByText(/Ask a project owner or your organization.s administrator for access/)).toBeInTheDocument();
  });

  it("gives every setup line a copy button", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <SetupPanel name="a" url={URL_OF} />
      </I18nextProvider>,
    );
    expect(screen.getAllByRole("button", { name: /Copy/ }).length).toBe(3);
  });
});
