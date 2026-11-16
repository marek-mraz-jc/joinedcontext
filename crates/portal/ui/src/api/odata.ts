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
import { api, ApiError, hiddenList, loginRedirectUrl, unwrap, type WireProblem } from "./client";
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
  jc_change_history: Answer<"list_change_history">;
  jc_endpoint_preview: Answer<"preview_endpoint">;
  jc_serviceaccount_access: Answer<"account_access">;
  jc_syncsource_status: Answer<"status">;
  jc_ckan_status: Answer<"get_status">;
}

/** Each action the UI calls whose answer is its REST door's, and that answer's shape. */
interface ActionAnswers {
  jc_syncsource_sync: RestAnswer<"sync_now">;
  jc_syncsource_pause: RestAnswer<"pause">;
  jc_datasource_discover: RestAnswer<"discover_datasource">;
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
): Promise<unknown> {
  return refusedAs(
    project === undefined
      ? client.organizationCall(name, params)
      : client.call(name, project, params),
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
>;

/**
 * Inside a workspace, a manifest is read and written in its branch (API/01 §22) through the REST
 * doors with `?workspace=` (client.ts workspaceMiddleware): the service reads and binds only what
 * the project holds, and a manifest drafted in the copy is not there.
 * ponytail: the service has no binding of a workspace's copy yet; T-7073's split adds one.
 */
const inCopy = {
  list: async (project: string, plural: string) =>
    unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural } } })),
  read: async (project: string, plural: string, name: string) =>
    unwrap(await api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path: { project, plural, name } } })),
  write: async (
    project: string,
    plural: string,
    body: { metadata: { name: string } },
    { create, dryRun, ifMatch, name }: { create: boolean; dryRun: boolean; ifMatch?: string; name: string },
  ): Promise<unknown> => {
    const query = dryRun ? { dryRun: "All" } : undefined;
    const manifest = body as components["schemas"]["ResourceProposal"];
    return unwrap(
      create
        ? await api.POST("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural }, query }, body: manifest })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural, name }, query, header: ifMatch ? { "If-Match": ifMatch } : undefined },
            body: manifest,
          }),
    );
  },
  remove: async (
    project: string,
    plural: string,
    name: string,
    { dryRun, ifMatch, confirm }: { dryRun: boolean; ifMatch?: string; confirm?: string },
  ): Promise<unknown> =>
    unwrap(
      await api.DELETE("/api/v1/projects/{project}/{plural}/{name}", {
        params: {
          path: { project, plural, name },
          query: { ...(dryRun ? { dryRun: "All" } : {}), ...(confirm ? { confirm } : {}) },
          header: ifMatch ? { "If-Match": ifMatch } : undefined,
        },
      }),
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
 * when the caller may not read its manifest; with `several`, or on an address that serves no one
 * Organization, the REST list (T-7104).
 */
export async function listOrganizations(several = false): Promise<{ items: Manifest[] }> {
  const rest = async () =>
    unwrap(
      await api.GET("/api/v1/projects/{project}/{plural}", {
        params: { path: { project: ORG_NAMESPACE, plural: "organizations" } },
      }),
    );
  // A person who sees several Organizations chooses among them (`?org=`, ADR-N-058 section 2.7):
  // the service serves one, so the REST list holds them all.
  if (several) return rest();
  let row: ManifestRow;
  try {
    row = (await client.singleton(ORGANIZATION, ["name", "spec", "status", "metadata"])) as ManifestRow;
  } catch (error) {
    // An address of several Organizations serves no one's service: the installation's
    // administrators choose among them there (`?org=`, ADR-N-058 section 2.7), so its REST list.
    if (error instanceof ODataError && error.code === SEVERAL_ORGANIZATIONS) return rest();
    throw toApiError(error);
  }
  return { items: row.metadata == null ? [] : [asManifest(row, ORGANIZATION)] };
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
