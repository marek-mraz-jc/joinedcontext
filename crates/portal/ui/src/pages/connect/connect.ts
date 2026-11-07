// The public page of one named MCP server (T-4141, T-4169, EP-92, ADR-N-043): its address, its
// tools as an anonymous caller sees them, and one ready setup per AI client. Nothing here holds a
// token: the page asks the server as a stranger would, and every setup names the address alone.
import { mcpOrigin } from "../../components/endpoints/links";
import { rpcAnswer, toolsOf } from "../mcp/mcp";
import type { Tool } from "../mcp/mcp";

/**
 * Anonymous calls a client IP may make to the named servers in a minute, the edge's
 * `context-mcp-named` limit (joinedcontext-deployment components/context-gateway/apisix-routes.yaml).
 */
export const ANONYMOUS_CALLS_PER_MINUTE = 60;

/** The protocol revision the page announces in `initialize`; the server answers with its own. */
const PROTOCOL_VERSION = "2025-06-18";

/**
 * The platform host an MCP client connects to: the domain the Portal is served under
 * (`portal.<domain>`), else this page's own origin, which serves the same route.
 */
export function serverUrl(hostname: string, project: string, name: string): string {
  const domain = hostname.startsWith("portal.") ? hostname.slice("portal.".length) : undefined;
  return `${mcpOrigin(domain)}/api/mcp/${encodeURIComponent(project)}/${encodeURIComponent(name)}`;
}

/** The name a client lists the server under: `{project}-{name}`, which every client accepts. */
export function clientName(project: string, name: string): string {
  return `${project}-${name}`.replace(/[^a-z0-9-]/gi, "-").toLowerCase();
}

/** One ready setup per client. Each names the address and nothing else: no token, no key. */
export function setups(url: string, server: string): { claudeCode: string; claudeDesktop: string; generic: string } {
  return {
    claudeCode: `claude mcp add --transport http ${server} ${url}`,
    // Claude Desktop's config file starts local servers only; `mcp-remote` bridges it to this one.
    claudeDesktop: JSON.stringify({ mcpServers: { [server]: { command: "npx", args: ["-y", "mcp-remote", url] } } }, null, 2),
    generic: JSON.stringify({ mcpServers: { [server]: { type: "http", url } } }, null, 2),
  };
}

export interface ServerInfo {
  title: string;
  instructions: string;
  tools: Tool[];
}

/** What the page shows: the server, or why it cannot. */
export type ServerAnswer =
  | { state: "answers"; server: ServerInfo }
  | { state: "missing" }
  | { state: "busy"; retryAfter?: number }
  | { state: "failed"; reason: string };

class Refused extends Error {
  constructor(readonly answer: ServerAnswer) {
    super("refused");
  }
}

/**
 * One JSON-RPC call to the server on this origin, as an anonymous caller: `credentials: "omit"`,
 * so a signed-in visitor sees what the page promises a stranger, not their own grants.
 */
async function call(url: string, method: string, params: object, signal?: AbortSignal): Promise<unknown> {
  const response = await globalThis.fetch(
    new Request(url, {
      method: "POST",
      credentials: "omit",
      headers: { "Content-Type": "application/json", Accept: "application/json, text/event-stream" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      signal,
    }),
  );
  if (response.status === 401 || response.status === 403 || response.status === 404) {
    throw new Refused({ state: "missing" });
  }
  if (response.status === 429) {
    const retryAfter = Number.parseInt(response.headers.get("retry-after") ?? "", 10);
    throw new Refused({ state: "busy", retryAfter: Number.isFinite(retryAfter) ? retryAfter : undefined });
  }
  if (!response.ok) throw new Refused({ state: "failed", reason: `HTTP ${response.status}` });
  const answer = rpcAnswer(response.headers.get("content-type") ?? "", await response.text());
  const error = (answer as { error?: { message?: unknown } } | null)?.error;
  if (error) throw new Refused({ state: "failed", reason: typeof error.message === "string" ? error.message : "error" });
  return answer;
}

/** The server's title, instructions and tools for an anonymous caller. */
export async function readServer(project: string, name: string, signal?: AbortSignal): Promise<ServerAnswer> {
  // Straight to the data host the setups name: the Portal host would answer an anonymous call
  // with a 308 there anyway, a hop the page's CSP and the edge's CORS each judge (T-4949).
  const url = serverUrl(window.location.hostname, project, name);
  try {
    const hello = (await call(
      url,
      "initialize",
      { protocolVersion: PROTOCOL_VERSION, capabilities: {}, clientInfo: { name: "joinedcontext-connect-page", version: "1" } },
      signal,
    )) as { result?: { serverInfo?: { name?: unknown; title?: unknown }; instructions?: unknown } } | null;
    const info = hello?.result?.serverInfo;
    const title = typeof info?.title === "string" ? info.title : typeof info?.name === "string" ? info.name : name;
    const instructions = typeof hello?.result?.instructions === "string" ? hello.result.instructions : "";
    const tools = toolsOf(await call(url, "tools/list", {}, signal));
    return { state: "answers", server: { title, instructions, tools } };
  } catch (error) {
    if (error instanceof Refused) return error.answer;
    if ((error as { name?: string }).name === "AbortError") throw error;
    return { state: "failed", reason: error instanceof Error ? error.message : "unreachable" };
  }
}
