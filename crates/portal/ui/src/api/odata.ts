// T-6812 (ADR-N-070, API/13 §2a, §2b): the Portal UI's one client of its Organization's OData
// service. Reads of sets, the functions and the actions go through here; the REST doors in
// client.ts shrink page group by page group under tests/odata_ratchet.test.ts.
import {
  odata,
  ODATA_ENTITY_KEYS,
  ODataError,
  originTransport,
  type ODataModel,
} from "@joinedcontext/sdk";
import { ApiError, hiddenList, loginRedirectUrl, type WireProblem } from "./client";
import { ORG_NAMESPACE } from "./manifest";
import { activeWorkspace } from "../components/layout/WorkspaceContext";
import type { components, operations } from "./schema";
import { announceSessionEnded } from "./sessionEnded";
import { announceStepUp, stepUpUrl } from "./stepUp";

/** The service root: the Portal serves its own Organization at its origin (API/15 §2). */
export const ODATA_ROOT = "/api/v1/";

const SECOND_FACTOR = "https://joinedcontext.com/errors/second-factor-required";
/** The service's refusal of an address that serves several Organizations (T-7104). */
const SEVERAL_ORGANIZATIONS = "several-organizations";

/** The problem type of an answer, an OData error object's or a REST problem's. */
function typeOf(body: unknown): unknown {
  if (typeof body !== "object" || body === null) return undefined;
  const error = (body as { error?: { type?: unknown } }).error;
  return error?.type ?? (body as { type?: unknown }).type;
}

/**
 * `fetch` with what every Portal request has: a 401 ends the session where the person is, and a
 * decision that needs a second factor opens the step-up dialog (T-2747, T-5567), as api/client.ts
 * does for the REST doors.
 */
async function portalFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  // One `Request` with an absolute address, as the REST client sends: jsdom has no base for a path.
  const request =
    input instanceof Request
      ? input
      : new Request(new URL(String(input), window.location.origin), init);
  const response = await globalThis.fetch(request);
  const here = `${window.location.pathname}${window.location.search}`;
  if (response.status === 401 && window.location.pathname !== "/login") {
    announceSessionEnded(loginRedirectUrl(here));
  } else if (response.status === 403) {
    const body: unknown = await response
      .clone()
      .json()
      .catch(() => null);
    if (typeOf(body) === SECOND_FACTOR) announceStepUp(stepUpUrl(here));
  }
  return response;
}

const client = odata(ODATA_ROOT, { transport: originTransport(portalFetch) });
/** The same service for a report sent as the tab closes: `keepalive` lets it finish (T-7101). */
const closingClient = odata(ODATA_ROOT, {
  transport: originTransport((input, init) =>
    portalFetch(input, { ...init, keepalive: true }),
  ),
});

/**
 * A refusal of the service as every page reads one: an ApiError whose problem holds the OData
 * error's API/09 members (`type`, `title`, `status`, `message` as `detail`), so a page shows its
 * refusal, empty and retry states unchanged.
 */
export function toApiError(error: unknown): unknown {
  if (!(error instanceof ODataError)) return error;
  // A bare failure (the edge's 502, a body that is no error object) says nothing to show: the
  // page says its own sentence for the status, as for a REST door's.
  if (!error.described) return new ApiError(error.status, error.message, undefined, error.requestId);
  const problem: WireProblem = {
    type: error.type,
    title: error.title,
    status: error.status,
    detail: error.message,
    // The fields a refusal names, as the REST problem's `errors` a page marks them by.
    ...(error.details.length
      ? {
          errors: error.details.map(({ target, message }) => ({
            path: target,
            message,
          })),
        }
      : {}),
    ...(error.hint ? { hint: error.hint } : {}),
  };
  const hidden = hiddenList(error.status, problem);
  return new ApiError(error.status, hidden?.detail ?? error.message, hidden ?? problem, error.requestId, hidden !== undefined);
}

async function refusedAs<T>(answer: Promise<T>): Promise<T> {
  try {
    return await answer;
  } catch (error) {
    throw toApiError(error);
  }
}

type Functions = ODataModel.Functions;
type Actions = ODataModel.Actions;

/** The answer an operation documents in the REST contract, until T-7075 retires the REST types. */
type Answer<O extends keyof operations> = operations[O] extends {
  responses: { 200: { content: { "application/json": infer T } } };
}
  ? T
  : never;

/** Each function the UI calls and the shape of its answer: the same handler serves its REST door. */
interface Answers {
  jc_access_explore: Answer<"explore_access">;
  jc_access_simulate: Answer<"simulate_access">;
  jc_catalogue_draft: Answer<"draft_publication">;
  jc_change_history: Answer<"list_change_history">;
  jc_endpoint_preview: Answer<"preview_endpoint">;
  jc_serviceaccount_access: Answer<"account_access">;
  jc_syncsource_status: Answer<"status">;
  jc_ckan_status: Answer<"get_status">;
  jc_workspace_compare: Answer<"compare_workspace">;
  jc_workspace_preview_get: Answer<"get_workspace_preview">;
}

/** Each action the UI calls whose answer is its REST door's, and that answer's shape. */
interface ActionAnswers {
  jc_syncsource_sync: RestAnswer<"sync_now">;
  jc_syncsource_pause: RestAnswer<"pause">;
  jc_datasource_discover: RestAnswer<"discover_datasource">;
  jc_workspace_open: RestAnswer<"open_workspace">;
  jc_workspace_update_from_main: RestAnswer<"update_workspace">;
  jc_workspace_preview_start: RestAnswer<"start_workspace_preview">;
  /** The Change it opened, under `change`, as every proposing operation answers (mutate.rs). */
  jc_workspace_propose: { changeId: string; lane: string; url?: string };
}

/**
 * A bound function of `project`, e.g. `call("jc_change_history", "air", { page: 1 })`: its value
 * is the operation's answer.
 */
export function call<F extends keyof Functions & string>(
  name: F,
  project: string,
  params: Functions[F],
): Promise<F extends keyof Answers ? Answers[F] : unknown> {
  return refusedAs(client.call(name, project, params)) as Promise<
    F extends keyof Answers ? Answers[F] : unknown
  >;
}

/** Each Administration page's reading of `jc_organization_read` and its answer (T-7098). */
interface Readings {
  setup: Answer<"get_setup">;
  health: Answer<"get_health">;
  healthChecks: Answer<"list_all">;
  limits: Answer<"get_limits">;
  brokers: Answer<"list_brokers">;
  agents: Answer<"list_agents">;
  agentActivity: Answer<"list_agent_activity">;
  costs: Answer<"get_costs">;
  addresses: Answer<"list_addresses">;
  dataAccess: Answer<"get_data_access">;
  widgetOptions: Answer<"list_options">;
  personReadable: Answer<"get_person_readable">;
}

type ReadingParams = Omit<Functions["jc_organization_read"], "aspect">;

/**
 * One reading of the Organization's Administration pages, bound to its `Organization` singleton,
 * e.g. `organizationReading("costs", { month: "2026-09" })`: the same handler serves its REST door.
 */
export function organizationReading<A extends keyof Readings>(
  aspect: A,
  params: ReadingParams = {},
): Promise<Readings[A]> {
  return refusedAs(
    client.organizationCall("jc_organization_read", { ...params, aspect }),
  ) as Promise<Readings[A]>;
}

/**
 * A bound action on `bound` (`projects('air')`, an entity, a Change), with `ifMatch` the entity's
 * `@odata.etag` where the action changes it.
 */
export function act<A extends keyof Actions & string>(
  name: A,
  bound: string,
  params: Actions[A],
  ifMatch?: string,
): Promise<A extends keyof ActionAnswers ? ActionAnswers[A] : unknown> {
  return refusedAs(client.act(name, bound, params, ifMatch)) as Promise<
    A extends keyof ActionAnswers ? ActionAnswers[A] : unknown
  >;
}

/**
 * The `@odata.etag` of project `project`'s row: the `If-Match` of a Change-proposing action bound
 * to it (API/13 §2b), e.g. `jc_syncsource_detach`.
 */
export async function projectEtag(project: string): Promise<string | undefined> {
  const row = (await refusedAs(client.get("projects", project))) as { "@odata.etag"?: string };
  return row["@odata.etag"];
}

/**
 * An OData text literal in an address: quoted, a quote doubled, percent-encoded so a `/`, `?` or
 * `#` in a name stays in its key (the SDK's keyPredicate does the same).
 */
function quoted(text: string): string {
  return encodeURIComponent(`'${text.replaceAll("'", "''")}'`);
}

/** The address an action of project `project` is bound to. */
export function projectBound(project: string): string {
  return `projects(${quoted(project)})`;
}

/** The `Organization` singleton the people's operations and the Organization's Portal doors are bound to. */
export const ORGANIZATION = "Organization";

/** The success body a REST operation documents, until T-7075 retires the REST types. */
type Success<R> = R extends {
  200: { content: { "application/json": infer T } };
}
  ? T
  : R extends { 201: { content: { "application/json": infer T } } }
    ? T
    : R extends { 202: { content: { "application/json": infer T } } }
      ? T
      : null;

/** What the REST operation `O` answers: the same handler answers its Portal door. */
export type RestAnswer<O extends keyof operations> = operations[O] extends {
  responses: infer R;
}
  ? Success<R>
  : never;

type Door<K> = K extends `portal_${string}` ? K : never;

/**
 * A Portal door's function (API/13 §2b, T-7100): a REST door no tool may answer, which the service
 * runs for a person's session only, bound to the Organization or, with `project`, to that project:
 * `door("portal_secret_list", {}, "air")`. A `204` of its REST door answers `null`.
 */
export function door<F extends Door<keyof Functions & string>>(
  name: F,
  params: Functions[F],
  project?: string,
  signal?: AbortSignal,
): Promise<unknown> {
  return refusedAs(
    project === undefined
      ? client.organizationCall(name, params, { signal })
      : client.call(name, project, params, { signal }),
  );
}

/**
 * A Portal door's action, e.g. `doorAction("portal_person_reset_password", { person: id })`;
 * `closing` sends it with `keepalive`, for a report the tab makes as it is hidden or closed.
 */
export async function doorAction<A extends Door<keyof Actions & string>>(
  name: A,
  params: Actions[A],
  project?: string,
  closing = false,
): Promise<unknown> {
  const value = await refusedAs(
    (closing ? closingClient : client).act(
      name,
      project === undefined ? ORGANIZATION : projectBound(project),
      params,
    ),
  );
  // The client reads a missing value as the whole body; a door's `null` stays `null`.
  return typeof value === "object" &&
    value !== null &&
    "@odata.context" in value
    ? null
    : value;
}

type EntitySets = ODataModel.EntitySets;

/** The sets whose rows are manifests. */
export type ManifestSet = Exclude<
  keyof EntitySets & string,
  | "changes"
  | "endpointList"
  | "entities"
  | "members"
  | "repositories"
  | "revisions"
  | "runs"
  | "workspaces"
>;

/** `dryRun` as the REST manifest doors read it: `All` checks, nothing writes. */
const checkOnly = (dryRun: boolean) => (dryRun ? "All" : undefined);

/** The workspace a page is drafting in, as a manifest door's `workspace`. */
const copyName = () => activeWorkspace() ?? undefined;

/**
 * A manifest write through the Portal's manifest doors (T-7177): inside a workspace in its copy
 * (API/01 §22), in the `org` namespace (a create, or a kind the Organization has no set of) on
 * the Organization, else in the project for a kind the service has no set of. Each runs the REST
 * door's handler and its check; `dryRun` checks without proposing.
 */
export function writeManifestDoor(
  project: string,
  plural: string,
  body: { metadata: { name: string } },
  { create, dryRun, ifMatch, name = body.metadata.name }: { create: boolean; dryRun: boolean; ifMatch?: string; name?: string },
): Promise<unknown> {
  const check = checkOnly(dryRun);
  if (project === ORG_NAMESPACE) {
    return create
      ? doorAction("portal_org_manifest_create", { plural, dryRun: check, body })
      : doorAction("portal_org_manifest_replace", { plural, name, dryRun: check, ifMatch, body });
  }
  const workspace = copyName();
  return create
    ? doorAction("portal_manifest_create", { plural, workspace, dryRun: check, body }, project)
    : doorAction("portal_manifest_replace", { plural, name, workspace, dryRun: check, ifMatch, body }, project);
}

/**
 * One page of a kind's list as the REST door searches and pages it (`q`, `count`, `cursor`), in
 * the workspace's copy when a page drafts in one: the grid's list (routes/kindPages.ts).
 */
export async function pageManifests(
  project: string,
  plural: string,
  { limit, q, cursor }: { limit: number; q?: string; cursor?: string },
): Promise<RestAnswer<"list">> {
  return (await door(
    "portal_manifest_list",
    { plural, workspace: copyName(), limit, count: true, q: q || undefined, cursor },
    project,
  )) as RestAnswer<"list">;
}

/**
 * Inside a workspace, a manifest is read and written in its branch (API/01 §22): the service's
 * sets read and bind only what the project holds, and a manifest drafted in the copy is not
 * there, so the manifest doors carry the copy's name.
 */
const inCopy = {
  list: async (project: string, plural: string) =>
    (await door("portal_manifest_list", { plural, workspace: copyName() }, project)) as { items: Manifest[] },
  read: async (project: string, plural: string, name: string) => {
    const { value, etag } = await refusedAs(
      client.callWithEtag("portal_manifest_get", project, { plural, name, workspace: copyName() }),
    );
    const manifest = value as Manifest;
    // The REST door's ETag, as a service row's `@odata.etag`, for the write's If-Match.
    if (etag && typeof manifest === "object" && manifest !== null) etags.set(manifest, etag);
    return manifest;
  },
  write: writeManifestDoor,
  remove: async (
    project: string,
    plural: string,
    name: string,
    { dryRun, ifMatch, confirm }: { dryRun: boolean; ifMatch?: string; confirm?: string },
  ): Promise<unknown> =>
    doorAction(
      "portal_manifest_delete",
      { plural, name, workspace: copyName(), dryRun: checkOnly(dryRun), confirm, ifMatch },
      project,
    ),
};

/** The sets of {@link ManifestSet}'s complement: their rows are no manifests. */
const NOT_MANIFESTS: readonly string[] = [
  "changes",
  "endpointList",
  "entities",
  "members",
  "repositories",
  "revisions",
  "runs",
  "workspaces",
];

/** Whether `plural` names a set whose rows are manifests, e.g. a kind page's plural. */
export function isManifestSet(plural: string): plural is ManifestSet {
  return Object.hasOwn(ODATA_ENTITY_KEYS, plural) && !NOT_MANIFESTS.includes(plural);
}

/** `plural` as the set of its manifests; a plural the service has no such set of is not found. */
function manifestSet(plural: string): ManifestSet {
  if (isManifestSet(plural)) return plural;
  const detail = `The service has no set of manifests called ${plural}.`;
  throw new ApiError(404, detail, { type: "about:blank", title: "Not Found", status: 404, detail });
}

type Manifest = components["schemas"]["ResourceEnvelope"];

/** The members a manifest set answers when `$select` names them (T-7097, API/13 §2a). */
const MANIFEST_MEMBERS = [
  "name",
  "kind",
  "metadata",
  "spec",
  "status",
  // A Pipeline is `v1alpha1` or `v1alpha2` (jc-core conversions): a write sends back what it read.
  "apiVersion",
] as const;

const API_VERSION = "joinedcontext.com/v1alpha1";

/** Each manifest a read answered and the `@odata.etag` of its row, for a write's `If-Match`. */
const etags = new WeakMap<object, string>();

/** The `@odata.etag` the service answered with `manifest`, a write's `If-Match`. */
export function manifestEtag(manifest: object): string | undefined {
  return etags.get(manifest);
}

interface ManifestRow {
  apiVersion?: unknown;
  kind?: unknown;
  metadata?: unknown;
  spec?: unknown;
  status?: unknown;
  "@odata.etag"?: string;
}

/**
 * The rows that carry no `kind` member: a `projects` row is the Project's own (T-7104), keyed by
 * name in the Organization's repository.
 */
const KIND_OF: Partial<Record<ManifestSet, string>> = { projects: "Project" };

/** The members `$select` names on `plural`'s rows. */
function membersOf(plural: ManifestSet): readonly string[] {
  return KIND_OF[plural]
    ? MANIFEST_MEMBERS.filter((m) => m !== "kind" && m !== "apiVersion")
    : MANIFEST_MEMBERS;
}

function asManifest(row: ManifestRow, kind?: string): Manifest {
  const manifest = {
    apiVersion: typeof row.apiVersion === "string" ? row.apiVersion : API_VERSION,
    kind: kind ?? row.kind,
    metadata: row.metadata,
    spec: row.spec ?? undefined,
    status: row.status ?? null,
  } as Manifest;
  if (row["@odata.etag"]) etags.set(manifest, row["@odata.etag"]);
  return manifest;
}

/**
 * Whether `plural`'s rows are keyed in a project: a set keyed by name alone is the Organization's
 * (Group, RoleBinding) and no project's navigation, which the service answers 400 (T-7118).
 */
function inProject(plural: ManifestSet): boolean {
  return (ODATA_ENTITY_KEYS[plural] as readonly string[]).includes("project");
}

/** The Organization set of the `org` namespace's `plural`: its own Roles are `organizationRoles`. */
function orgSet(plural: ManifestSet): ManifestSet {
  return plural === "roles" ? "organizationRoles" : plural;
}

/**
 * Every manifest of `plural` in `project` the caller may read, as the REST door's
 * `GET /projects/{project}/{plural}` lists them (`items`): the project's navigation, or for the
 * `org` namespace the Organization set of that name, with `$select` of the manifest's members.
 */
export async function listManifests(
  project: string,
  of: string,
): Promise<{ items: Manifest[] }> {
  if (activeWorkspace()) return inCopy.list(project, of);
  const plural = manifestSet(of);
  // The REST door listed the project's namespace, where an Organization kind is never stored.
  if (project !== ORG_NAMESPACE && !inProject(plural)) return { items: [] };
  // `$select` names manifest members (metadata, spec) the generated row types of a set leave out.
  const query: { select(...members: string[]): { all(): AsyncIterable<unknown> } } =
    project === ORG_NAMESPACE
      ? client.from(orgSet(plural))
      : client.under(project, plural);
  const items: Manifest[] = [];
  const kind = KIND_OF[plural];
  const rows = query.select(...membersOf(plural)).all();
  await refusedAs(
    (async () => {
      for await (const row of rows) {
        // A project's member reads its row but not its manifest, which the REST list leaves out.
        if ((row as ManifestRow).metadata == null) continue;
        items.push(asManifest(row as ManifestRow, kind));
      }
    })(),
  );
  return { items };
}

/** A manifest the caller may not read, refused as the REST door refuses it. */
function notFound(name: string): ApiError {
  const detail = `Your roles do not let you read the settings of ${name}, or it no longer exists.`;
  return new ApiError(404, detail, { type: "about:blank", title: "Not Found", status: 404, detail });
}

/**
 * The manifest `name` of `plural` in `project`, as the REST door's
 * `GET /projects/{project}/{plural}/{name}` answers it; its row's ETag in {@link manifestEtag}.
 */
export async function readManifest(
  project: string,
  of: string,
  name: string,
): Promise<Manifest> {
  if (activeWorkspace()) return inCopy.read(project, of, name);
  const plural = manifestSet(of);
  if (project !== ORG_NAMESPACE && !inProject(plural)) throw notFound(name);
  const members = membersOf(plural);
  const row = (await refusedAs(
    project === ORG_NAMESPACE
      ? client.get(orgSet(plural), name, members)
      : client.get(plural, { project, name }, members),
  )) as ManifestRow;
  if (row.metadata == null) throw notFound(name);
  return asManifest(row, KIND_OF[plural]);
}

/**
 * The Organization this service serves, as the REST door's `GET /projects/org/organizations`
 * lists it (`items`): its `Organization` singleton with `$select` of the manifest's members, none
 * when the caller may not read its manifest; on an address that serves no one Organization, the
 * REST list (T-7104).
 */
export async function listOrganizations(): Promise<{ items: Manifest[] }> {
  const rest = async () => (await door("portal_organization_list", {})) as { items: Manifest[] };
  let row: ManifestRow;
  try {
    row = (await client.singleton(ORGANIZATION, ["name", "spec", "status", "metadata"])) as ManifestRow;
  } catch (error) {
    // An address of several Organizations serves no one's service: the installation's own
    // pages read the Organizations the person may (a door of their own, which needs no one
    // Organization) and pick the installation's from them.
    if (error instanceof ODataError && error.code === SEVERAL_ORGANIZATIONS) return rest();
    throw toApiError(error);
  }
  return { items: row.metadata == null ? [] : [asManifest(row, ORGANIZATION)] };
}



type WorkspaceView = components["schemas"]["WorkspaceView"];

/**
 * The members of a Workspace row the pages read; `scope` and `changes` are its detail, answered
 * only when `$select` names them, and a list's `changes` stays null as the REST list's does.
 */
const WORKSPACE_MEMBERS = [
  "name",
  "title",
  "project",
  "owner",
  "branch",
  "baseRevision",
  "scope",
  "previewState",
  "createdAt",
  "expiresAt",
] as const;

/** Every copy of `project` the caller sees, as the REST door's `GET …/workspaces` lists them. */
export async function listWorkspaces(project: string): Promise<{ items: WorkspaceView[] }> {
  // `$select` names `scope`, a member the generated row type leaves out.
  const query: { select(...members: string[]): { all(): AsyncIterable<unknown> } } = client.under(
    project,
    "workspaces",
  );
  const items: WorkspaceView[] = [];
  const rows = query.select(...WORKSPACE_MEMBERS).all();
  await refusedAs(
    (async () => {
      for await (const row of rows) items.push(row as WorkspaceView);
    })(),
  );
  return { items };
}

/**
 * The copy `name` of `project` with how many files it changes, as the REST door's
 * `GET …/workspaces/{name}` answers it.
 */
export async function readWorkspace(project: string, name: string): Promise<WorkspaceView> {
  return (await refusedAs(
    client.get("workspaces", { project, name }, [...WORKSPACE_MEMBERS, "changes"]),
  )) as WorkspaceView;
}

/**
 * A run's members, as the REST door's `GET …/agent-runs/{id}` answers them: the `runs` row's
 * scalar fields and the detail the service answers when `$select` names it (T-7134).
 */
const RUN_MEMBERS = [
  "id",
  "project",
  "appName",
  "kind",
  "status",
  "createdBy",
  "createdAt",
  "previewUrl",
  "title",
  "endpointName",
  "endpointSlug",
  "endpoints",
  "profile",
  "unattended",
  "continues",
  "appClass",
  "visibility",
  "prompt",
  "promptDigest",
  "dataNeeds",
  "allowsWrite",
  "branch",
  "pathPrefix",
  "workspace",
  "mergeRequest",
  "changeId",
  "sourceUrl",
  "mirrorUrl",
  "firstFrameMs",
  "firstVersionMs",
  "steps",
  "tokensUsed",
  "origin",
  "startedAt",
  "finishedAt",
  "expiresAt",
  "error",
] as const;

/** What {@link listRuns} narrows to, each the REST list's query parameter of that name. */
export interface RunFilter {
  app?: string;
  kind?: string;
  /** Only the caller's own runs: `me` is the caller's username, as `/me` answers it. */
  createdBy?: string;
  /** `all` adds the live journeys' test runs to people's runs (AG-93); people's only when absent. */
  origin?: "person" | "all";
  top?: number;
}

/**
 * The runs of `project` the caller sees, newest first, as the REST door's `GET …/agent-runs`
 * lists them: the `runs` set with `$filter` of `filter` (T-7134). The set holds the newest 100
 * of each origin, the REST list's own ceiling, and a run's `origin` keeps the journeys' test
 * runs out unless `filter.origin` asks for them (T-7145).
 */
/** One revision of a project's history: the sha an export reads from (MF-16). */
export type Revision = ODataModel.EntitySets["revisions"] & { message: string; author: string; date?: string | null };

/**
 * A project's newest `top` revisions, newest first, as the export dialog lists them (the
 * `revisions` set, T-7177); throws the ApiError of a refusal (503 while the forge is away).
 */
export async function listRevisions(project: string, top: number): Promise<{ items: Revision[] }> {
  const page = await refusedAs(client.under(project, "revisions").top(top).page());
  return { items: page.value as Revision[] };
}

export async function listRuns<R = unknown>(project: string, filter: RunFilter = {}): Promise<{ items: R[] }> {
  const where = (
    [
      ["appName", filter.app],
      ["kind", filter.kind],
      ["createdBy", filter.createdBy],
      ["origin", filter.origin === "all" ? undefined : "person"],
    ] as const
  )
    .filter(([, value]) => value !== undefined && value !== "")
    .map(([member, value]) => `${member} eq '${String(value).replace(/'/g, "''")}'`);
  // `$select` names the detail members the generated row type leaves out.
  let query: {
    select(...members: string[]): typeof query;
    filter(expression: string): typeof query;
    top(n: number): typeof query;
    orderBy(member: string, direction: "asc" | "desc"): typeof query;
    all(): AsyncIterable<unknown>;
  } = client.under(project, "runs");
  query = query.select(...RUN_MEMBERS).orderBy("createdAt", "desc");
  if (where.length > 0) query = query.filter(where.join(" and "));
  if (filter.top !== undefined) query = query.top(filter.top);
  const items: R[] = [];
  const rows = query.all();
  await refusedAs(
    (async () => {
      for await (const row of rows) items.push(row as R);
    })(),
  );
  return { items };
}

/** Run `id` of `project`, one of the caller's, as the REST door's `GET …/agent-runs/{id}` answers it. */
export async function readRun<R = unknown>(project: string, id: string): Promise<R> {
  return (await refusedAs(client.get("runs", { project, id }, RUN_MEMBERS))) as R;
}

type ChangeProposal = components["schemas"]["ChangeProposal"];

/** Every member of the REST door's `GET …/changes/{id}`: each the detail of the Change's row. */
const CHANGE_MEMBERS = [
  "apiVersion",
  "kind",
  "metadata",
  "status",
  "summary",
  "author",
  "createdAt",
  "ci",
  "fileCount",
  "files",
  "planFields",
  "waitsOn",
  "workspace",
  "decision",
] as const;

type ChangeList = components["schemas"]["ChangeList"];

/** What only one Change's read holds: its list item leaves these out, as the REST list does. */
const CHANGE_READ_ONLY: readonly string[] = ["ci", "files", "planFields"];

/**
 * The open Changes of `project`, newest first, as the REST door's `GET …/changes` lists them: the
 * `changes` set with `$select` of its items' members, which the service answers from the list
 * itself (T-7126); the `org` namespace's own Changes, which the set leaves out, through their
 * Portal door. The merge requests outside the Changes are {@link listOutside}'s.
 */
export async function listChanges(project: string): Promise<ChangeList> {
  if (project === ORG_NAMESPACE) return listChangesOfOrg();
  const query: { select(...members: string[]): { all(): AsyncIterable<unknown> } } = client.under(
    project,
    "changes",
  );
  const items: ChangeProposal[] = [];
  const rows = query.select(...CHANGE_MEMBERS.filter((member) => !CHANGE_READ_ONLY.includes(member))).all();
  await refusedAs(
    (async () => {
      for await (const row of rows) items.push(row as ChangeProposal);
    })(),
  );
  items.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  return { apiVersion: API_VERSION, kind: "ChangeList", items };
}

/**
 * The merge requests of `project`'s repository nobody can approve here, a fork's or a
 * non-writer's, each with why: listed to an organization administrator, to anyone else none.
 */
export async function listOutside(project: string): Promise<ChangeList["outside"]> {
  if (project === ORG_NAMESPACE) return (await listChangesOfOrg()).outside;
  return ((await door("portal_change_outside", {}, project)) as { items: NonNullable<ChangeList["outside"]> }).items;
}

/** The `org` namespace's own Changes with their merge requests outside, in one read. */
async function listChangesOfOrg(): Promise<ChangeList> {
  return (await door("portal_organization_change_list", {})) as ChangeList;
}

/**
 * Change `id` of `project`, open or decided, as the REST door's `GET …/changes/{id}` answers it:
 * the `changes` row by its key with `$select` of its detail (T-7122); the `org` namespace's own
 * Changes, which the set leaves out, through their Portal door.
 */
export async function readChange(project: string, id: string): Promise<ChangeProposal> {
  if (project === ORG_NAMESPACE)
    return (await door("portal_organization_change_get", { id })) as ChangeProposal;
  return (await refusedAs(
    client.get("changes", { project, name: id }, CHANGE_MEMBERS),
  )) as unknown as ChangeProposal;
}

/**
 * Approves Change `id` of `project`, with `confirm` the name typed back for a red-lane removal
 * (CC-39): the Change's `approve` action, or the `org` namespace's Portal door. `If-Match: *`
 * decides it as the REST door did; the handler refuses a Change no longer waiting.
 */
export async function approveChange(
  project: string,
  id: string,
  confirm?: string,
): Promise<RestAnswer<"approve_change">> {
  const params = confirm === undefined ? {} : { confirm };
  return (await (project === ORG_NAMESPACE
    ? doorAction("portal_organization_change_approve", { id, body: params })
    : act("approve", changeBound(project, id), params, "*"))) as RestAnswer<"approve_change">;
}

/** Rejects Change `id` of `project` with `reason`, which its proposer reads: as {@link approveChange}. */
export async function rejectChange(
  project: string,
  id: string,
  reason: string,
): Promise<RestAnswer<"reject_change">> {
  return (await (project === ORG_NAMESPACE
    ? doorAction("portal_organization_change_reject", { id, body: { reason } })
    : act("reject", changeBound(project, id), { reason }, "*"))) as RestAnswer<"reject_change">;
}

type ResourceList = components["schemas"]["ResourceList"];

/** The manifests merged Change `id` of `project` wrote, through its Portal door (T-3975, T-7126). */
export async function changeManifests(project: string, id: string): Promise<ResourceList> {
  return (await (project === ORG_NAMESPACE
    ? door("portal_organization_change_manifests", { id })
    : door("portal_change_manifests", { id }, project))) as ResourceList;
}

/**
 * Proposes again what merged Change `id` of `project` removed (`restore`, T-3247) or altered
 * (`undo`, T-3274), through its Portal door: the new Change waits for an approver.
 */
export async function decidedChange(
  verb: "restore" | "undo",
  project: string,
  id: string,
): Promise<RestAnswer<"restore_change">> {
  return (await (project === ORG_NAMESPACE
    ? doorAction(`portal_organization_change_${verb}`, { id })
    : doorAction(`portal_change_${verb}`, { id }, project))) as RestAnswer<"restore_change">;
}

/** The address of Change `id` of `project`, the entity `approve` and `reject` are bound to. */
function changeBound(project: string, id: string): string {
  return `changes(project=${quoted(project)},name=${quoted(id)})`;
}

/** The address of manifest `name` of `plural` in `project`, the entity its write actions are bound to. */
export function manifestBound(project: string, plural: ManifestSet, name: string): string {
  if (project !== ORG_NAMESPACE && !inProject(plural)) throw notFound(name);
  return project === ORG_NAMESPACE
    ? `${orgSet(plural)}(${quoted(name)})`
    : `${plural}(project=${quoted(project)},name=${quoted(name)})`;
}

/**
 * The `If-Match` of a write: the ETag the page read the manifest with, else `*`, which writes as
 * the REST door's PUT and DELETE without If-Match did (API/13 section 3).
 */
function matching(ifMatch: string | undefined): string {
  return ifMatch ?? "*";
}

/**
 * Proposes `manifest` of `plural` in `project` as a Change, or checks it with `dryRun` (PF-57):
 * `propose_create` on the project, `propose_update` on the manifest with `If-Match` (API/13 §2a).
 * The answer is the REST door's: a Change, or with `dryRun` the check's verdict.
 */
export async function proposeManifest(
  project: string,
  of: string,
  manifest: { metadata: { name: string } },
  {
    create,
    dryRun = false,
    ifMatch,
    name = manifest.metadata.name,
  }: {
    create: boolean;
    dryRun?: boolean;
    ifMatch?: string;
    /** The manifest an update replaces, when the page addresses it by another name than the body's. */
    name?: string;
  },
): Promise<unknown> {
  if (activeWorkspace()) return inCopy.write(project, of, manifest, { create, dryRun, ifMatch, name });
  const plural = manifestSet(of);
  if (create) {
    return refusedAs(client.act("propose_create", projectBound(project), { manifest, dryRun }));
  }
  const bound = manifestBound(project, plural, name);
  return refusedAs(client.act("propose_update", bound, { manifest, dryRun }, matching(ifMatch)));
}

/**
 * Proposes deleting manifest `name` as a Change, or checks it with `dryRun` (`propose_delete`);
 * `confirm` is the name typed back, with which an administrator's own red-lane removal is
 * approved as it is proposed (PF-58, CC-39).
 */
export async function proposeDelete(
  project: string,
  of: string,
  name: string,
  { dryRun = false, ifMatch, confirm }: { dryRun?: boolean; ifMatch?: string; confirm?: string } = {},
): Promise<unknown> {
  if (activeWorkspace()) return inCopy.remove(project, of, name, { dryRun, ifMatch, confirm });
  const bound = manifestBound(project, manifestSet(of), name);
  return refusedAs(
    client.act("propose_delete", bound, { dryRun, ...(confirm ? { confirm } : {}) }, matching(ifMatch)),
  );
}
