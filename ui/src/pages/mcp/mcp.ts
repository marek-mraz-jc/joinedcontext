// Named MCP servers over chosen Endpoints (MF-53, EP-92…EP-96, ADR-N-043, T-3156): the manifest a
// form proposes, the address and client configuration a person copies, and the live look at each
// member's own MCP surface the page shows before and after a server is proposed.
import { readCsrfToken } from "../../api/client";
import type { Manifest, ResourceProposal } from "../../api/manifest";

/** The most member Endpoints one server may name (ADR-N-043 §2.7). */
export const MAX_MEMBERS = 10;

export const AUDIENCES = ["project-list", "organization", "public"] as const;
export type Audience = (typeof AUDIENCES)[number];

/** How wide an audience is: a server is no wider than its narrowest member (MF-53). */
export function breadth(audience: string | undefined): number {
  const index = AUDIENCES.indexOf(audience as Audience);
  return index < 0 ? 0 : index;
}

/** One Endpoint a person may pick as a member, from a project's list they may read. */
export interface MemberChoice {
  project: string;
  name: string;
  title: string;
  slug: string;
  audience: Audience;
  /** `spec.mcp: false` serves no MCP surface (EP-24), so it cannot be a member (MF-53). */
  servesMcp: boolean;
}

/** `{project}/{name}`, the key a member is picked and shown by. */
export function memberKey(member: { project: string; name: string }): string {
  return `${member.project}/${member.name}`;
}

/** An Endpoint manifest of `project` as a choice. */
export function asChoice(project: string, endpoint: Manifest, title: string): MemberChoice | null {
  const spec = (endpoint.spec ?? {}) as { slug?: unknown; audience?: unknown; mcp?: unknown };
  if (typeof spec.slug !== "string" || spec.slug === "") return null;
  const audience = AUDIENCES.includes(spec.audience as Audience) ? (spec.audience as Audience) : "project-list";
  return { project, name: endpoint.metadata.name, title, slug: spec.slug, audience, servesMcp: spec.mcp !== false };
}

/** What the form holds. */
export interface McpServerForm {
  name: string;
  title: string;
  description: string;
  audience: Audience;
  allowedProjects: string[];
  /** Picked members as `{project}/{name}`, in the order they were picked. */
  members: string[];
  /** Attached knowledge sources, `{source}` of this project or `{project}/{source}` (MF-57). */
  knowledgeSources: string[];
  /** `spec.select` as stored (MF-55): the Endpoints a server selects beside its members, kept as is. */
  select?: Record<string, unknown>;
}

/** The most knowledge sources one server attaches (MF-57). */
export const MAX_KNOWLEDGE_SOURCES = 20;

export function emptyForm(): McpServerForm {
  return { name: "", title: "", description: "", audience: "project-list", allowedProjects: [], members: [], knowledgeSources: [] };
}

/** The widest audience the picked members allow: the narrowest member's own (MF-53). */
export function widestAllowed(members: MemberChoice[]): Audience {
  if (members.length === 0) return "project-list";
  return AUDIENCES[Math.min(...members.map((member) => breadth(member.audience)))];
}

/** What is wrong with a form before anything is checked on the server, one key per field. */
export function formProblems(form: McpServerForm, picked: MemberChoice[]): Record<string, string> {
  const problems: Record<string, string> = {};
  if (!/^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/.test(form.name)) problems.name = "mcp.problem.name";
  // A server names members, a `select`, or both; the server judges a select against the audience (MF-55).
  if ((form.members.length === 0 && !form.select) || form.members.length > MAX_MEMBERS) problems.members = "mcp.problem.members";
  else if (picked.some((member) => !member.servesMcp)) problems.members = "mcp.problem.noMcp";
  if (picked.length > 0 && breadth(form.audience) > breadth(widestAllowed(picked))) problems.audience = "mcp.problem.audience";
  if (form.knowledgeSources.length > MAX_KNOWLEDGE_SOURCES) problems.knowledgeSources = "mcp.problem.knowledgeSources";
  if (form.audience === "project-list" && form.allowedProjects.some((p) => !/^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/.test(p))) {
    problems.allowedProjects = "mcp.problem.projects";
  }
  return problems;
}

/** The manifest a form proposes; a stored one keeps the metadata the form does not show. */
export function toManifest(project: string, form: McpServerForm, stored?: Manifest): ResourceProposal {
  const kept = (stored?.metadata ?? {}) as Record<string, unknown>;
  const metadata: Record<string, unknown> = { ...kept, name: form.name, namespace: project };
  if (form.title.trim()) metadata.title = { ...((kept.title as Record<string, unknown> | undefined) ?? {}), en: form.title.trim() };
  else delete metadata.title;
  if (form.description.trim()) {
    metadata.description = { ...((kept.description as Record<string, unknown> | undefined) ?? {}), en: form.description.trim() };
  } else delete metadata.description;
  const spec: Record<string, unknown> = {
    members: form.members.map((key) => {
      const [namespace, name] = key.split("/");
      return { kind: "Endpoint", name, namespace };
    }),
    audience: form.audience,
  };
  if (form.select) spec.select = form.select;
  if (form.audience === "project-list" && form.allowedProjects.length > 0) spec.allowedProjects = form.allowedProjects;
  if (form.knowledgeSources.length > 0) spec.knowledgeSources = form.knowledgeSources;
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "McpServer", metadata, spec } as ResourceProposal;
}

function english(value: unknown): string {
  if (typeof value === "string") return value;
  if (value && typeof value === "object") {
    const en = (value as Record<string, unknown>).en;
    if (typeof en === "string") return en;
  }
  return "";
}

/** A stored server as the form shows it; a member without `namespace` is in the server's project. */
export function fromManifest(project: string, manifest: Manifest): McpServerForm {
  const spec = (manifest.spec ?? {}) as {
    members?: unknown;
    audience?: unknown;
    allowedProjects?: unknown;
    knowledgeSources?: unknown;
    select?: unknown;
  };
  const members = (Array.isArray(spec.members) ? spec.members : [])
    .map((member) => member as { name?: unknown; namespace?: unknown })
    .filter((member) => typeof member.name === "string")
    .map((member) => `${typeof member.namespace === "string" ? member.namespace : project}/${member.name as string}`);
  return {
    name: manifest.metadata.name,
    title: english(manifest.metadata.title),
    description: english(manifest.metadata.description),
    audience: AUDIENCES.includes(spec.audience as Audience) ? (spec.audience as Audience) : "project-list",
    allowedProjects: Array.isArray(spec.allowedProjects) ? spec.allowedProjects.filter((p): p is string => typeof p === "string") : [],
    members,
    knowledgeSources: Array.isArray(spec.knowledgeSources)
      ? spec.knowledgeSources.filter((s): s is string => typeof s === "string")
      : [],
    ...(spec.select && typeof spec.select === "object" && !Array.isArray(spec.select)
      ? { select: spec.select as Record<string, unknown> }
      : {}),
  };
}

/** The Keycloak client a connector signs in with (EP-96): public, PKCE, no secret. */
export function clientId(project: string, name: string): string {
  return `mcp-${project}-${name}`;
}

/**
 * The `mcpServers` entry a person pastes into an AI client, as the catalogue's snippets write it:
 * the address and nothing else. The client finds the sign-in through the server's protected-
 * resource metadata (EP-96) and signs the person in with {@link clientId}; no token is written.
 */
export function clientConfig(url: string, server: string): string {
  return JSON.stringify({ mcpServers: { [server]: { type: "http", url } } }, null, 2);
}

export interface Tool {
  name: string;
  description: string;
  readOnly: boolean;
}

/** One member's own MCP surface, as this person reaches it now. */
export type Probe =
  | { state: "answers"; tools: Tool[] }
  | { state: "refused"; status: number }
  | { state: "failed"; reason: string };

/** The JSON-RPC answer of a Streamable HTTP POST: plain JSON, or the first `data:` of an event stream. */
export function rpcAnswer(contentType: string, body: string): unknown {
  if (contentType.includes("text/event-stream")) {
    const data = body
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trim())[0];
    return data ? JSON.parse(data) : null;
  }
  return JSON.parse(body);
}

/** The tools of a `tools/list` answer. */
export function toolsOf(answer: unknown): Tool[] {
  const tools = (answer as { result?: { tools?: unknown } } | null)?.result?.tools;
  return (Array.isArray(tools) ? tools : [])
    .map((tool) => tool as { name?: unknown; description?: unknown; annotations?: { readOnlyHint?: unknown } })
    .filter((tool) => typeof tool.name === "string")
    .map((tool) => ({
      name: tool.name as string,
      description: typeof tool.description === "string" ? tool.description : "",
      readOnly: tool.annotations?.readOnlyHint === true,
    }));
}

/**
 * Asks one member's MCP surface for its tools on this origin, where the edge turns the person's
 * session into the bearer: exactly what the member grants this person, nothing the server adds.
 */
export async function probe(slug: string, signal?: AbortSignal): Promise<Probe> {
  try {
    const response = await globalThis.fetch(
      new Request(`${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}/mcp`, {
        method: "POST",
        credentials: "same-origin",
        headers: {
          "Content-Type": "application/json",
          Accept: "application/json, text/event-stream",
          "x-csrf-token": readCsrfToken() ?? "",
        },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }),
        signal,
      }),
    );
    if (response.status === 401 || response.status === 403 || response.status === 404) {
      return { state: "refused", status: response.status };
    }
    if (!response.ok) return { state: "failed", reason: `HTTP ${response.status}` };
    const answer = rpcAnswer(response.headers.get("content-type") ?? "", await response.text());
    const error = (answer as { error?: { message?: unknown } } | null)?.error;
    if (error) return { state: "failed", reason: typeof error.message === "string" ? error.message : "error" };
    return { state: "answers", tools: toolsOf(answer) };
  } catch (error) {
    if ((error as { name?: string }).name === "AbortError") throw error;
    return { state: "failed", reason: error instanceof Error ? error.message : "unreachable" };
  }
}

/**
 * The server's catalogue for this person (ADR-N-043 §2.3): a tool is listed when at least one
 * member that answers offers it, with the members that do. A refused member adds nothing.
 */
export function mergedTools(probes: { member: string; probe: Probe }[]): { tool: Tool; members: string[] }[] {
  const byName = new Map<string, { tool: Tool; members: string[] }>();
  for (const { member, probe: result } of probes) {
    if (result.state !== "answers") continue;
    for (const tool of result.tools) {
      const entry = byName.get(tool.name) ?? { tool, members: [] };
      entry.members.push(member);
      byName.set(tool.name, entry);
    }
  }
  return [...byName.values()].sort((a, b) => a.tool.name.localeCompare(b.tool.name));
}

/** One Endpoint's NGSI-LD query card as this person reaches it (T-4760). */
export type CardRead = { state: "card"; text: string } | { state: "refused"; status: number };

/**
 * Reads the gateway's `card://{slug}` resource on this origin, where the edge turns the person's
 * session into the bearer: the very text every MCP client is served, for this person's grants.
 * A 401 or 403, and an Endpoint whose card the gateway does not serve them, are refusals.
 */
export async function readCard(slug: string, signal?: AbortSignal): Promise<CardRead> {
  const response = await globalThis.fetch(
    new Request(`${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}/mcp`, {
      method: "POST",
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
        "x-csrf-token": readCsrfToken() ?? "",
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "resources/read", params: { uri: `card://${slug}` } }),
      signal,
    }),
  );
  if (response.status === 401 || response.status === 403 || response.status === 404) {
    return { state: "refused", status: response.status };
  }
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const answer = rpcAnswer(response.headers.get("content-type") ?? "", await response.text()) as {
    error?: unknown;
    result?: { contents?: { text?: unknown }[]; isError?: unknown };
  } | null;
  if (answer?.error) return { state: "refused", status: 403 };
  const text = answer?.result?.contents?.[0]?.text;
  if (answer?.result?.isError === true || typeof text !== "string") {
    throw new Error("the card could not be read");
  }
  return { state: "card", text };
}
