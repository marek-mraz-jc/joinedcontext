// A person's AI connectors (AG-142..AG-153, API/14, T-5951): each an MCP server at `/c/{id}` that
// reaches only its scope, read again against the person's own rights at every call.
import type { components } from "../../api/schema";

export type AiConnector = components["schemas"]["ConnectorGrant"];
export type ConnectorOptions = components["schemas"]["ConnectorOptions"];
export type ScopeItem = components["schemas"]["ConnectorScopeItem"];
export type NewAiConnector = components["schemas"]["NewConnector"];
export type KindVerbs = components["schemas"]["ConnectorKindVerbs"];

/** A change verb a person ticks per kind (T-6302); `read` comes with the preset. */
export type ChangeVerb = "create" | "edit" | "delete" | "run";
const CHANGE_VERBS: ChangeVerb[] = ["create", "edit", "delete", "run"];

/** The verbs a kind offers: `run` only on Pipeline. */
export function verbsOf(kind: string): ChangeVerb[] {
  return CHANGE_VERBS.filter((verb) => verb !== "run" || kind === "Pipeline");
}

/** The kinds the ticked presets reach, each once, under the first preset that reaches it. */
export function kindRows(options: ConnectorOptions, segments: string[]): { segment: string; kinds: string[] }[] {
  const seen = new Set<string>();
  return options.segmentKinds
    .filter((row) => segments.includes(row.segment))
    .map((row) => ({ segment: row.segment, kinds: row.kinds.filter((kind) => !seen.has(kind) && seen.add(kind)) }))
    .filter((row) => row.kinds.length > 0);
}

/** Whether the ceiling lets a connector tick `verb` on `kind`. */
export function ceilingAllows(options: ConnectorOptions, kind: string, verb: ChangeVerb): boolean {
  return options.ceiling.kinds.some((one) => one.kind === kind && one.verbs.includes(verb));
}

/** What a tick in the tree names: a whole project, or one space, Endpoint or pipeline of it. */
export type PickKind = "project" | "space" | "endpoint" | "pipeline";
export interface Pick {
  project: string;
  kind: PickKind;
  name?: string;
  access: "read" | "write";
}

export interface AiConnectorForm {
  name: string;
  segments: string[];
  picks: Pick[];
  /** Change verbs per kind beyond read; a kind not here is read only. */
  verbs: Record<string, ChangeVerb[]>;
  /** `YYYY-MM-DD`; the connector ends at the end of that day, UTC. */
  expires: string;
  clients: string[];
  /** Approve each change it applies at once, as the owner (T-6552); only with a write pick. */
  autonomous: boolean;
}

/**
 * The locale key that names a connector client, from a fixed map of known ids (T-6309): a CIMD URL
 * is a client's published document, whose own text never becomes a label. Unknown ids show as is.
 */
const CLIENT_KEYS: Record<string, string> = {
  "https://claude.ai/oauth/mcp-oauth-client-metadata": "claudeAi",
  "https://claude.ai/oauth/mcp-client-metadata": "claudeAi",
  "https://claude.ai/oauth/claude-code-client-metadata": "claudeCode",
  "https://chatgpt.com/connector/oauth/client.json": "chatgpt",
  "mcp-local": "mcp-local",
  "mcp-hub": "mcp-hub",
  "mcp-mobile": "mcp-mobile",
};

/** The words for a client id, e.g. `claude.ai` for its CIMD URL; the id itself when unknown. */
export function clientLabel(id: string, t: (key: string) => string): string {
  const key = Object.hasOwn(CLIENT_KEYS, id) ? CLIENT_KEYS[id] : undefined;
  return key ? t(`aiConnectors.client.${key}`) : id;
}

/** A clients' refusal without its field path, each offered id replaced by its words, e.g. `the client Claude Code`. */
export function namedClients(message: string, clients: { id: string }[], t: (key: string) => string): string {
  const longestFirst = [...clients].sort((a, b) => b.id.length - a.id.length);
  const text = message.replace(/^allowedClients(\[\d+\])?: /, "");
  return longestFirst.reduce((named, one) => named.split(one.id).join(clientLabel(one.id, t)), text);
}

/**
 * A clients' refusal in plain words: which client it names and which the organization allows. A
 * refusal naming no offered client keeps the server's text, each offered id in words.
 */
export function clientRefusal(
  message: string,
  clients: { id: string; refused?: string | null }[],
  t: (key: string, values?: Record<string, string>) => string,
): { id?: string; text: string } {
  const id = clients.find((one) => message.endsWith(`does not allow the client ${one.id}`))?.id;
  if (!id) return { text: namedClients(message, clients, t) };
  const allowed = clients.filter((one) => !one.refused && one.id !== id).map((one) => clientLabel(one.id, t));
  const client = clientLabel(id, t);
  return { id, text: t(allowed.length > 0 ? "aiConnectors.clientsAllowedOnly" : "aiConnectors.noClientAllowed", { client, clients: allowed.join(", ") }) };
}

/** The refusal's words while the client it names stays ticked: Create would only be refused again. */
export function refusalHolding(
  message: string | undefined,
  form: AiConnectorForm,
  clients: { id: string; refused?: string | null }[],
  t: (key: string, values?: Record<string, string>) => string,
): string | undefined {
  const said = message ? clientRefusal(message, clients, t) : undefined;
  return said?.id && form.clients.includes(said.id) ? said.text : undefined;
}

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;
const DAY = 24 * 3600_000;

export function pickKey(pick: Pick): string {
  return pick.kind === "project" ? pick.project : `${pick.project}/${pick.kind}/${pick.name ?? ""}`;
}

/** The last day a connector made today may live, as the date input reads it. */
export function lastDay(maxDays: number, now: Date): string {
  return new Date(now.getTime() + (maxDays - 1) * DAY).toISOString().slice(0, 10);
}

export function emptyForm(options: ConnectorOptions | undefined, now: Date): AiConnectorForm {
  return {
    name: "",
    segments: options?.ceiling.segments.slice(0, 1) ?? [],
    picks: [],
    verbs: {},
    expires: lastDay(options?.ceiling.maxDays ?? 30, now),
    clients: [],
    autonomous: false,
  };
}

/** The verbs a stored list holds; `propose` stands for create, edit and delete. */
function heldVerbs(kinds: KindVerbs[]): Record<string, ChangeVerb[]> {
  const verbs: Record<string, ChangeVerb[]> = {};
  for (const one of kinds) {
    const held = one.verbs.flatMap((verb) => (verb === "propose" ? ["create", "edit", "delete"] : [verb]));
    const picked = verbsOf(one.kind).filter((verb) => held.includes(verb));
    if (picked.length > 0) verbs[one.kind] = picked;
  }
  return verbs;
}

/**
 * The form a stored connector fills: the edit starts from what it holds. A connector made before
 * T-6302 holds no list and changes every kind its presets reach: the form shows that as every
 * verb `options` lets it tick, so saving keeps what it could do.
 */
export function toForm(connector: AiConnector, options?: ConnectorOptions): AiConnectorForm {
  const legacy = (): Record<string, ChangeVerb[]> =>
    Object.fromEntries(
      (options ? kindRows(options, connector.segments) : [])
        .flatMap((row) => row.kinds)
        .map((kind) => [kind, verbsOf(kind).filter((verb) => options !== undefined && ceilingAllows(options, kind, verb))]),
    );
  return {
    name: connector.name,
    segments: connector.segments,
    picks: scopeLines(connector.scope).map((line) => ({ project: line.where, kind: line.kind, name: line.name, access: line.access === "write" ? "write" : "read" })),
    verbs: connector.kinds ? heldVerbs(connector.kinds) : legacy(),
    expires: connector.expiresAt.slice(0, 10),
    clients: connector.allowedClients,
    autonomous: connector.autonomous,
  };
}

/**
 * The request the form asks for; the server checks every item of it again. `kinds` names every
 * kind the presets reach, `read` alone where nothing more is ticked (deny by default).
 */
export function toRequest(form: AiConnectorForm, options?: ConnectorOptions): NewAiConnector {
  const scope: ScopeItem[] = form.picks.map((pick) => ({
    project: pick.project,
    ...(pick.kind === "project" ? {} : { [pick.kind]: pick.name }),
    access: pick.access,
  }));
  return {
    name: form.name,
    segments: form.segments,
    scope,
    kinds: (options ? kindRows(options, form.segments) : [])
      .flatMap((row) => row.kinds)
      .map((kind) => ({ kind, verbs: ["read", ...verbsOf(kind).filter((verb) => form.verbs[kind]?.includes(verb))] })),
    writes: form.picks.some((pick) => pick.access === "write"),
    autonomous: form.autonomous && form.picks.some((pick) => pick.access === "write"),
    expiresAt: `${form.expires}T23:59:59Z`,
    allowedClients: form.clients,
  };
}

/** The form's own refusals, as translation keys per section, before the server is asked. */
export function formProblems(form: AiConnectorForm, options: ConnectorOptions, now: Date): Record<string, string> {
  const problems: Record<string, string> = {};
  if (!DNS_LABEL.test(form.name)) problems.name = "aiConnectors.problem.name";
  if (form.segments.length === 0) problems.segments = "aiConnectors.problem.segments";
  if (form.picks.length === 0) problems.scope = "aiConnectors.problem.scope";
  const today = now.toISOString().slice(0, 10);
  if (!form.expires || form.expires < today || form.expires > lastDay(options.ceiling.maxDays, now)) {
    problems.expiresAt = "aiConnectors.problem.expires";
  }
  return problems;
}

/** The form section a server refusal names: `errors[0]`, or the field a 403's detail starts with. */
export function refusedSection(errors: string[] | undefined, detail: string): string | undefined {
  const field = errors?.[0] ?? /^([A-Za-z]+)(\[\d+\])?[.:]/.exec(detail)?.[1];
  const head = field?.replace(/\[.*$/, "");
  if (head === "writes") return "scope";
  return head && ["name", "segments", "kinds", "scope", "expiresAt", "allowedClients", "autonomous"].includes(head) ? head : undefined;
}

/** One line per scope item, as the list shows it: `helsinki`, `helsinki / Endpoint air`. */
export function scopeLines(scope: ScopeItem[]): { where: string; kind: PickKind; name?: string; access: string }[] {
  return scope.map((item) => {
    const kind: PickKind = item.space ? "space" : item.endpoint ? "endpoint" : item.pipeline ? "pipeline" : "project";
    return { where: item.project, kind, name: item.space ?? item.endpoint ?? item.pipeline ?? undefined, access: item.access };
  });
}

/** Whether a connector reads only organization data: every item in a project the person reaches as shared datasets alone. */
export function readsShared(connector: AiConnector, options: ConnectorOptions | undefined): boolean {
  const shared = (options?.projects ?? []).filter((project) => project.organizationData).map((project) => project.project);
  return connector.scope.length > 0 && connector.scope.every((item) => shared.includes(item.project));
}

/** What a person pastes into each AI client (API/14 §4). */
export function setup(name: string, url: string): { claudeCode: string; vscode: string } {
  return {
    claudeCode: `claude mcp add --transport http ${name} ${url}`,
    vscode: JSON.stringify({ servers: { [name]: { type: "http", url } } }, null, 2),
  };
}
