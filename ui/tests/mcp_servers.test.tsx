// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/mcp/mcp.ts,
// src/pages/mcp/McpServersPage.tsx, src/pages/mcp/McpServerDialog.tsx and
// src/pages/mcp/McpServerPanels.tsx, the named MCP servers of a project (T-3156, MF-53, ADR-N-043).
import type { ReactNode } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import {
  breadth,
  clientConfig,
  clientId,
  emptyForm,
  formProblems,
  fromManifest,
  mergedTools,
  rpcAnswer,
  toManifest,
  toolsOf,
  widestAllowed,
} from "../src/pages/mcp/mcp";
import type { Manifest } from "../src/api/manifest";
import { McpServerDialog } from "../src/pages/mcp/McpServerDialog";
import { CopyValue, ToolPreview } from "../src/pages/mcp/McpServerPanels";
import { McpServersPage } from "../src/pages/mcp/McpServersPage";
import type { MemberChoice } from "../src/pages/mcp/mcp";

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", roles: [] };

const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "steward", binding: "people", scope: "project:helsinki", rule: { kinds: ["McpServer", "Endpoint"], verbs } }],
});

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (project: string, name: string, slug: string, audience: string, extra: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: project, title: { en: name.replace(/-/g, " ") } },
  spec: { slug, audience, contextSpaceRef: { name: project }, ...extra },
});

const SERVER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "McpServer",
  metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility data" }, description: { en: "Bikes and parking." } },
  spec: {
    members: [
      { kind: "Endpoint", name: "bikes", namespace: "helsinki" },
      { kind: "Endpoint", name: "parking", namespace: "espoo" },
    ],
    audience: "organization",
  },
};

const TOOLS = (names: string[]) => ({
  jsonrpc: "2.0",
  id: 1,
  result: { tools: names.map((name) => ({ name, description: `${name} of the endpoint`, annotations: { readOnlyHint: name !== "create_entity" } })) },
});

interface Stub {
  verbs?: string[];
  servers?: unknown[];
  /** The dry run refuses, naming this field (T-4948). */
  refuseField?: string;
}

function renderAt(path: string, stub: Stub = {}) {
  const writes: { path: string; body: unknown }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, code = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status: code, headers: { "Content-Type": code >= 400 ? "application/problem+json" : type } }));
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", domain: "dev.example", languages: { default: "en", offered: ["en"] } });
    if (url.pathname.endsWith("/permissions/me")) return json(grants(stub.verbs ?? ["read", "propose", "delete"]));
    // The members' own MCP surfaces, as the person reaches them: one answers as an event stream,
    // one as JSON, one refuses this person.
    if (url.pathname === "/api/endpoint/bikeslug/mcp") {
      return Promise.resolve(new Response(`event: message\ndata: ${JSON.stringify(TOOLS(["list_types", "query_entities", "create_entity"]))}\n\n`, { status: 200, headers: { "Content-Type": "text/event-stream" } }));
    }
    if (url.pathname === "/api/endpoint/parkslug/mcp") return json(TOOLS(["list_types", "query_entities"]));
    if (url.pathname === "/api/endpoint/hiddenslug/mcp") return json({ title: "Forbidden", status: 403 }, 403);
    if (request.method !== "GET") {
      writes.push({ path: url.pathname + url.search, body: request.body ? await request.json() : null });
      if (stub.refuseField) {
        return json({ title: "Bad Request", status: 400, detail: "spec.allowedProjects names project 'espoo', which is not one you may read; pick one from the list", errors: [stub.refuseField] }, 400);
      }
      if (url.searchParams.get("dryRun") === "All") return json({ valid: true, verdict: { ok: true, findings: [] } });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-mcp-1", namespace: "helsinki" }, spec: { lane: "Yellow" }, status: { phase: "Proposed" } }, 202);
    }
    if (url.pathname === "/api/v1/projects") return json(list([{ name: "helsinki" }, { name: "espoo" }]));
    if (url.pathname === "/api/v1/projects/helsinki/endpoints") {
      return json(list([endpoint("helsinki", "bikes", "bikeslug", "public"), endpoint("helsinki", "archive", "archslug", "organization", { mcp: false })]));
    }
    if (url.pathname === "/api/v1/projects/espoo/endpoints") {
      return json(list([endpoint("espoo", "parking", "parkslug", "organization"), endpoint("espoo", "staff", "hiddenslug", "project-list")]));
    }
    if (url.pathname === "/api/v1/projects/helsinki/mcpservers") return json(list(stub.servers ?? [SERVER]));
    if (url.pathname === "/api/v1/projects/helsinki/knowledge/sources") {
      return json({ items: [
        { source: "web", type: "website", state: "crawled", startUrls: ["https://www.hel.fi/"], visibility: "public" },
        { source: "intranet", type: "website", state: "crawled", startUrls: ["https://intra.hel.fi/"], visibility: "internal" },
      ] });
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  Object.assign(navigator, { clipboard: { writeText: vi.fn(() => Promise.resolve()) } });
  window.history.pushState({}, "", path);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { writes, fetchMock };
}

describe("the MCP servers page (T-3156)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows a server's address, sign-in client and entry, its members' health and the tools they offer this person", async () => {
    const { fetchMock } = renderAt("/projects/helsinki/mcp");
    const card = await screen.findByRole("region", { name: "Mobility data" });
    expect(within(card).getByText("https://api.dev.example/api/mcp/helsinki/mobility")).toBeInTheDocument();
    expect(within(card).getByText("mcp-helsinki-mobility")).toBeInTheDocument();
    const entry = within(card).getByLabelText("Entry for the AI client's configuration");
    expect(JSON.parse(entry.textContent ?? "")).toEqual({ mcpServers: { mobility: { type: "http", url: "https://api.dev.example/api/mcp/helsinki/mobility" } } });
    const members = within(card).getByRole("list", { name: "Members and how they answer you" });
    expect(await within(members).findByText("Answers, tools: 3")).toBeInTheDocument();
    expect(await within(members).findByText("Answers, tools: 2")).toBeInTheDocument();
    const tools = within(card).getByRole("list", { name: "Tools of the server" });
    // list_types and query_entities come from both members, create_entity from one.
    expect(within(tools).getAllByText("Offered by: helsinki/bikes, espoo/parking")).toHaveLength(2);
    expect(within(tools).getByText("Offered by: helsinki/bikes")).toBeInTheDocument();
    expect(within(tools).getByText("create_entity")).toBeInTheDocument();
    // No token is ever sent anywhere but as the page's own session, and none is written down.
    expect(entry.textContent).not.toMatch(/token|Bearer|secret/i);
    const probes = fetchMock.mock.calls.map(([input]) => (input as Request).url).filter((u) => u.includes("/api/endpoint/"));
    expect(probes.every((u) => u.startsWith(`${window.location.origin}/api/endpoint/`))).toBe(true);
  });

  it("links a public server to its connect page and keeps an organization server off it (T-4871)", async () => {
    renderAt("/projects/helsinki/mcp", { servers: [{ ...SERVER, metadata: { ...SERVER.metadata, name: "open", title: { en: "Open data" } }, spec: { ...SERVER.spec, audience: "public" } }, SERVER] });
    const open = await screen.findByRole("region", { name: "Open data" });
    expect(within(open).getByRole("link", { name: "Open the public connect page" })).toHaveAttribute("href", "/connect/helsinki/open");
    const internal = await screen.findByRole("region", { name: "Mobility data" });
    expect(within(internal).queryByRole("link", { name: "Open the public connect page" })).toBeNull();
  });

  it("creates a server over two Endpoints of two projects as a checked Change", async () => {
    const { writes } = renderAt("/projects/helsinki/mcp", { servers: [] });
    await userEvent.click(await screen.findByRole("button", { name: "New MCP server" }));
    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "mobility");
    await userEvent.type(within(dialog).getByLabelText(/^Title/), "Mobility data");
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: "bikes (helsinki/bikes)" }));
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "parking (espoo/parking)" }));
    const audience = within(dialog).getByLabelText(/^Who may connect/);
    // The narrowest member is organization-wide: public is not on offer.
    expect(within(audience).getByRole("option", { name: "Public" })).toBeDisabled();
    await userEvent.selectOptions(audience, "organization");
    expect(await within(dialog).findAllByText("Offered by: helsinki/bikes, espoo/parking")).toHaveLength(2);
    await userEvent.click(within(dialog).getByRole("button", { name: "Check and propose" }));
    expect(await within(dialog).findByText("chg-mcp-1", { exact: false })).toBeInTheDocument();
    const proposed = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "McpServer",
      metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility data" } },
      spec: {
        members: [
          { kind: "Endpoint", name: "bikes", namespace: "helsinki" },
          { kind: "Endpoint", name: "parking", namespace: "espoo" },
        ],
        audience: "organization",
      },
    };
    expect(writes).toEqual([
      { path: "/api/v1/projects/helsinki/mcpservers?dryRun=All", body: proposed },
      { path: "/api/v1/projects/helsinki/mcpservers", body: proposed },
    ]);
  });

  it("admits projects picked from the person's own list, and shows the server's refusal beside that field (T-4948)", async () => {
    const { writes } = renderAt("/projects/helsinki/mcp", { servers: [] });
    await userEvent.click(await screen.findByRole("button", { name: "New MCP server" }));
    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "shared");
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: "bikes (helsinki/bikes)" }));
    await userEvent.selectOptions(within(dialog).getByLabelText(/^Who may connect/), "project-list");
    const admits = within(dialog).getByLabelText(/^Projects it admits/);
    expect(admits).toHaveAccessibleDescription(/only projects you may read are listed/);
    await userEvent.type(admits, "zzz");
    expect(await screen.findByText(/No project you may read matches/)).toBeInTheDocument();
    await userEvent.clear(admits);
    await userEvent.type(admits, "esp");
    await userEvent.click(await screen.findByRole("option", { name: /espoo/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Check and propose" }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect((writes[0]?.body as { spec: { allowedProjects?: string[] } }).spec.allowedProjects).toEqual(["espoo"]);
  });

  it("shows the server's refusal of an admitted project beside the field (T-4948)", async () => {
    renderAt("/projects/helsinki/mcp", { servers: [], refuseField: "spec.allowedProjects" });
    await userEvent.click(await screen.findByRole("button", { name: "New MCP server" }));
    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "shared");
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: "bikes (helsinki/bikes)" }));
    await userEvent.selectOptions(within(dialog).getByLabelText(/^Who may connect/), "project-list");
    await userEvent.click(within(dialog).getByLabelText(/^Projects it admits/));
    await userEvent.click(await screen.findByRole("option", { name: /espoo/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Check and propose" }));
    await waitFor(() => expect(within(dialog).getByLabelText(/^Projects it admits/)).toHaveAccessibleDescription(/not one you may read/));
    expect(within(dialog).getAllByText(/not one you may read/)).toHaveLength(1);
  });

  it("attaches the project's public knowledge sources only, and keeps one it does not list (MF-57, T-4659)", async () => {
    const { writes } = renderAt("/projects/helsinki/mcp", { servers: [] });
    await userEvent.click(await screen.findByRole("button", { name: "New MCP server" }));
    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "weby");
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: "bikes (helsinki/bikes)" }));
    const web = await within(dialog).findByRole("checkbox", { name: "web" });
    expect(within(dialog).queryByRole("checkbox", { name: "intranet" })).not.toBeInTheDocument();
    await userEvent.click(web);
    expect(within(dialog).getByText("Knowledge sources (1 of at most 20)")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Check and propose" }));
    expect(await within(dialog).findByText("chg-mcp-1", { exact: false })).toBeInTheDocument();
    expect((writes[0].body as { spec: Record<string, unknown> }).spec.knowledgeSources).toEqual(["web"]);

    const stored = fromManifest("helsinki", { ...SERVER, spec: { ...SERVER.spec, knowledgeSources: ["web", "bbsk/web", 7] } } as unknown as Manifest);
    expect(stored.knowledgeSources).toEqual(["web", "bbsk/web"]);
    const again = toManifest("helsinki", { ...stored, knowledgeSources: [] }) as unknown as { spec: Record<string, unknown> };
    expect(again.spec.knowledgeSources).toBeUndefined();
  });

  it("keeps a select through the form: select only, members only and both round-trip (JC2-27)", () => {
    const select = { audience: ["public"], domains: ["mobility"] };
    const selectOnly = { ...SERVER, spec: { audience: "public", select } } as unknown as Manifest;
    const form = fromManifest("helsinki", selectOnly);
    expect(form.members).toEqual([]);
    expect(form.select).toEqual(select);
    expect(formProblems(form, [])).toEqual({});
    const saved = toManifest("helsinki", form) as unknown as { spec: Record<string, unknown> };
    expect(saved.spec).toEqual({ members: [], audience: "public", select });

    const both = fromManifest("helsinki", { ...SERVER, spec: { ...SERVER.spec, select } } as unknown as Manifest);
    expect((toManifest("helsinki", both) as unknown as { spec: Record<string, unknown> }).spec.select).toEqual(select);
    expect(both.members).toHaveLength(2);

    const membersOnly = fromManifest("helsinki", SERVER as unknown as Manifest);
    expect(membersOnly.select).toBeUndefined();
    expect((toManifest("helsinki", membersOnly) as unknown as { spec: Record<string, unknown> }).spec).not.toHaveProperty("select");
    // Neither members nor a select is still refused.
    expect(formProblems({ ...form, select: undefined }, []).members).toBe("mcp.problem.members");
  });

  it("refuses to propose without a member and says why, and an Endpoint without MCP cannot be picked", async () => {
    const { writes } = renderAt("/projects/helsinki/mcp", { servers: [] });
    await userEvent.click(await screen.findByRole("button", { name: "New MCP server" }));
    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "empty");
    const archive = await within(dialog).findByRole("checkbox", { name: "archive (helsinki/archive)" });
    expect(archive).toHaveAccessibleDescription(/serves no MCP surface/);
    await userEvent.click(within(dialog).getByRole("button", { name: "Check and propose" }));
    expect(await within(dialog).findByText("Pick between one and 10 member Endpoints.")).toBeInTheDocument();
    expect(writes).toEqual([]);
  });

  it("finds an Endpoint by its name and says when nothing matches", async () => {
    renderAt("/projects/helsinki/mcp", { servers: [] });
    await userEvent.click(await screen.findByRole("button", { name: "New MCP server" }));
    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    await within(dialog).findByRole("checkbox", { name: "parking (espoo/parking)" });
    await userEvent.type(within(dialog).getByLabelText("Find an Endpoint"), "park");
    // The members' checkboxes, `title (project/name)`; the knowledge sources have their own.
    expect(within(dialog).getAllByRole("checkbox", { name: /\(.+\/.+\)$/ })).toHaveLength(1);
    await userEvent.clear(within(dialog).getByLabelText("Find an Endpoint"));
    await userEvent.type(within(dialog).getByLabelText("Find an Endpoint"), "nothing-like-this");
    expect(within(dialog).getByText("No Endpoint matches, or none you may read.")).toBeInTheDocument();
  });

  it("a member this person may not read is kept unseen, and its refusal is named only for the members they pick", async () => {
    renderAt("/projects/helsinki/mcp", {
      servers: [{ ...SERVER, spec: { ...SERVER.spec, members: [...SERVER.spec.members, { kind: "Endpoint", name: "secret-one", namespace: "elsewhere" }] } }],
    });
    const card = await screen.findByRole("region", { name: "Mobility data" });
    expect(within(card).getByText("Members you may not read, kept as they are: 1")).toBeInTheDocument();
    expect(within(card).queryByText(/secret-one/)).toBeNull();
  });

  it("a viewer meets New disabled with the reason, and no edit or remove", async () => {
    renderAt("/projects/helsinki/mcp", { verbs: ["read"] });
    const create = await screen.findByRole("button", { name: "New MCP server" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "true"));
    const card = await screen.findByRole("region", { name: "Mobility data" });
    await waitFor(() => expect(within(card).getByRole("button", { name: "Remove mobility" })).toHaveAttribute("aria-disabled", "true"));
  });

  it("says the servers could not be read instead of claiming there are none", async () => {
    renderAt("/projects/helsinki/mcp");
    vi.mocked(globalThis.fetch).mockImplementation(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      if (url.pathname === "/api/v1/projects/helsinki/mcpservers") {
        return new Response(JSON.stringify({ title: "Service Unavailable", status: 503, detail: "the repository is not reachable" }), {
          status: 503,
          headers: { "Content-Type": "application/problem+json" },
        });
      }
      if (url.pathname.endsWith("/auth/me")) return new Response(JSON.stringify(IDENTITY), { headers: { "Content-Type": "application/json" } });
      return new Response(JSON.stringify(list([])), { headers: { "Content-Type": "application/json" } });
    });
    window.history.pushState({}, "", "/projects/helsinki/mcp?again=1");
    expect(await screen.findByText(/The MCP servers could not be read/)).toBeInTheDocument();
  });
});

describe("the named server's rules (MF-53)", () => {
  const choice = (project: string, name: string, audience: MemberChoice["audience"], servesMcp = true): MemberChoice => ({
    project,
    name,
    title: name,
    slug: `${name}slug`,
    audience,
    servesMcp,
  });

  it("is no wider than its narrowest member", () => {
    expect(widestAllowed([])).toBe("project-list");
    expect(widestAllowed([choice("a", "x", "public"), choice("b", "y", "public")])).toBe("public");
    expect(widestAllowed([choice("a", "x", "public"), choice("b", "y", "organization")])).toBe("organization");
    expect(breadth("project-list")).toBeLessThan(breadth("organization"));
    expect(breadth("nonsense")).toBe(0);
  });

  it("names every problem a form has before anything is sent", () => {
    const form = { ...emptyForm(), name: "Bad_Name", audience: "public" as const, members: ["a/x"] };
    expect(formProblems(form, [choice("a", "x", "organization")])).toEqual({ name: "mcp.problem.name", audience: "mcp.problem.audience" });
    expect(formProblems({ ...form, name: "ok", audience: "organization" }, [choice("a", "x", "organization", false)])).toEqual({ members: "mcp.problem.noMcp" });
    const eleven = Array.from({ length: 11 }, (_, i) => `a/m${i}`);
    expect(formProblems({ ...emptyForm(), name: "ok", members: eleven }, [])).toEqual({ members: "mcp.problem.members" });
    expect(formProblems({ ...emptyForm(), name: "ok", members: ["a/x"], allowedProjects: ["Not OK"] }, [choice("a", "x", "public")])).toEqual({
      allowedProjects: "mcp.problem.projects",
    });
    const many = Array.from({ length: 21 }, (_, i) => `s${i}`);
    expect(formProblems({ ...emptyForm(), name: "ok", members: ["a/x"], knowledgeSources: many }, [choice("a", "x", "public")])).toEqual({
      knowledgeSources: "mcp.problem.knowledgeSources",
    });
  });

  it("round-trips a manifest, keeps stored metadata and drops allowedProjects off a project-list", () => {
    const stored = { ...SERVER, metadata: { ...SERVER.metadata, labels: { team: "mobility" } } } as unknown as Manifest;
    const form = fromManifest("helsinki", stored);
    expect(form).toEqual({
      name: "mobility",
      title: "Mobility data",
      description: "Bikes and parking.",
      audience: "organization",
      allowedProjects: [],
      members: ["helsinki/bikes", "espoo/parking"],
      knowledgeSources: [],
    });
    const again = toManifest("helsinki", { ...form, allowedProjects: ["espoo"] }, stored) as unknown as Manifest & { spec: Record<string, unknown> };
    expect(again.metadata).toMatchObject({ labels: { team: "mobility" }, title: { en: "Mobility data" } });
    expect(again.spec.allowedProjects).toBeUndefined();
    const listed = toManifest("helsinki", { ...form, audience: "project-list", allowedProjects: ["espoo"] }) as unknown as { spec: Record<string, unknown> };
    expect(listed.spec.allowedProjects).toEqual(["espoo"]);
    // A member without a namespace is in the server's own project.
    expect(fromManifest("helsinki", { ...stored, spec: { members: [{ kind: "Endpoint", name: "bikes" }], audience: "public" } } as unknown as Manifest).members).toEqual(["helsinki/bikes"]);
  });

  it("reads a tools/list answer from JSON or an event stream, and merges only members that answer", () => {
    const streamed = rpcAnswer("text/event-stream", `event: message\ndata: ${JSON.stringify(TOOLS(["query_entities"]))}\n\n`);
    expect(toolsOf(streamed)).toEqual([{ name: "query_entities", description: "query_entities of the endpoint", readOnly: true }]);
    expect(toolsOf(rpcAnswer("application/json", JSON.stringify({ jsonrpc: "2.0", id: 1, result: {} })))).toEqual([]);
    expect(toolsOf(null)).toEqual([]);
    const merged = mergedTools([
      { member: "a/x", probe: { state: "answers", tools: toolsOf(TOOLS(["list_types", "query_entities"])) } },
      { member: "b/y", probe: { state: "answers", tools: toolsOf(TOOLS(["query_entities"])) } },
      { member: "c/z", probe: { state: "refused", status: 403 } },
    ]);
    expect(merged.map((entry) => [entry.tool.name, entry.members])).toEqual([
      ["list_types", ["a/x"]],
      ["query_entities", ["a/x", "b/y"]],
    ]);
  });

  it("writes the client and the entry without a token", () => {
    expect(clientId("helsinki", "mobility")).toBe("mcp-helsinki-mobility");
    expect(JSON.parse(clientConfig("https://api.dev.example/api/mcp/helsinki/mobility", "mobility"))).toEqual({
      mcpServers: { mobility: { type: "http", url: "https://api.dev.example/api/mcp/helsinki/mobility" } },
    });
  });
});

describe("the parts on their own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });
  const wrap = (node: ReactNode) =>
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
      </QueryClientProvider>,
    );

  it("copies a value and says it did", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    wrap(<CopyValue label="Address" value="https://api.dev.example/api/mcp/helsinki/mobility" />);
    await userEvent.click(screen.getByRole("button", { name: "Copy: Address" }));
    expect(writeText).toHaveBeenCalledWith("https://api.dev.example/api/mcp/helsinki/mobility");
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });

  it("asks for a member before it previews anything", () => {
    wrap(<ToolPreview members={[]} />);
    expect(screen.getByText("Pick a member to see the tools.")).toBeInTheDocument();
  });

  it("edits a stored server with its name fixed and an unreadable member kept", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(TOOLS([])), { headers: { "Content-Type": "application/json" } })));
    const stored = { ...SERVER, spec: { ...SERVER.spec, members: [...SERVER.spec.members, { kind: "Endpoint", name: "hidden", namespace: "elsewhere" }] } } as unknown as Manifest;
    wrap(
      <McpServerDialog
        project="helsinki"
        stored={stored}
        choices={[{ project: "helsinki", name: "bikes", title: "bikes", slug: "bikeslug", audience: "public", servesMcp: true }]}
        choicesLoading={false}
        open
        onOpenChange={() => undefined}
      />,
    );
    const dialog = screen.getByRole("dialog", { name: "Edit mobility" });
    expect(within(dialog).getByLabelText(/^Name/)).toHaveAttribute("readonly");
    expect(within(dialog).getByText("Members you may not read, kept as they are: 2")).toBeInTheDocument();
    expect(within(dialog).getByRole("checkbox", { name: "bikes (helsinki/bikes)" })).toBeChecked();
  });

  it("as the organization's tab it sits under the page's h1, and its servers live at /api/mcp/org", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url);
        if (url.pathname === "/api/v1/projects/org/mcpservers") {
          return new Response(JSON.stringify(list([{ ...SERVER, metadata: { ...SERVER.metadata, namespace: "org", name: "city" } }])), {
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response(JSON.stringify(list([])), { headers: { "Content-Type": "application/json" } });
      }),
    );
    wrap(<McpServersPage project="org" embedded />);
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    expect(screen.getByRole("heading", { level: 2, name: "MCP servers" })).toBeInTheDocument();
    expect(await screen.findByText(/\/api\/mcp\/org\/city$/)).toBeInTheDocument();
    expect(screen.getByText("mcp-org-city")).toBeInTheDocument();
  });

  it("the page asks for the project's servers and every member list it may offer", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      if (url.pathname === "/api/v1/projects") return new Response(JSON.stringify(list([{ name: "helsinki" }])), { headers: { "Content-Type": "application/json" } });
      return new Response(JSON.stringify(list([])), { headers: { "Content-Type": "application/json" } });
    });
    vi.stubGlobal("fetch", fetchMock);
    wrap(<McpServersPage project="helsinki" />);
    expect(await screen.findByText("This project has no MCP server yet")).toBeInTheDocument();
    await waitFor(() => {
      const asked = fetchMock.mock.calls.map(([input]) => new URL((input as Request).url).pathname);
      expect(asked).toEqual(expect.arrayContaining(["/api/v1/projects/helsinki/mcpservers", "/api/v1/projects/helsinki/endpoints"]));
    });
  });
});
