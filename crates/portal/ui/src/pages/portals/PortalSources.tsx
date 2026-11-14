import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { useOrganizations } from "../../api/projects";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  EmptyState,
  Field,
  InlineError,
  Input,
  PageFailed,
  PageLoading,
  Select,
} from "../../components/ui";
import type { BadgeTone } from "../../components/ui";
import type { PortalSpec } from "./PortalsArea";

/** The source types a Portal knows (docs/kinds/Portal.md, ODP-10), the harvested ones after `endpoints`. */
export const SOURCE_TYPES = ["endpoints", "dcat-ap", "ckan-action", "nkod-sparql", "arcgis-hub"] as const;
export type SourceType = (typeof SOURCE_TYPES)[number];

/** One `spec.sources[]` entry, the members this tab writes; the others stay as stored. */
export interface PortalSourceSpec {
  name: string;
  type: SourceType;
  url?: string;
  schedule?: string;
  allowedHosts?: string[];
  budget?: { records: number; bytes: number };
  projects?: string[];
  [member: string]: unknown;
}

/** The words the server answers a test or a failed run with (ODP-15), each with its locale key. */
const OUTCOMES = [
  "ok",
  "unreachable",
  "refused host",
  "not the expected format",
  "too large",
  "over budget",
  "past the time cap",
  "failed",
] as const;

/** The locale key of an outcome or run error word; an unknown word reads as `failed`. */
export function outcomeKey(word: string): string {
  const known = (OUTCOMES as readonly string[]).includes(word) ? word : "failed";
  return `portals.outcome.${known.replaceAll(" ", "_")}`;
}

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;
const CRON = /^\S+(\s+\S+){4}$/;
const HOST = /^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$/;

/** What the add form holds while a person types. */
export interface SourceDraft {
  name: string;
  type: SourceType | "";
  url: string;
  schedule: string;
  allowedHosts: string;
  records: string;
  bytes: string;
  projects: string[];
}

export const EMPTY_SOURCE: SourceDraft = {
  name: "",
  type: "",
  url: "",
  schedule: "23 3 * * *",
  allowedHosts: "",
  records: "5000",
  bytes: "50000000",
  projects: [],
};

type SourceField = "name" | "type" | "url" | "schedule" | "allowedHosts" | "records" | "bytes" | "projects";

function hostOf(url: string): string | undefined {
  try {
    return new URL(url).hostname.toLowerCase();
  } catch {
    return undefined;
  }
}

function hostsOf(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((host) => host.trim().toLowerCase())
    .filter((host) => host !== "");
}

/** What the add form refuses before anything is sent, by field, as locale keys. */
export function sourceFaults(draft: SourceDraft, taken: string[]): Partial<Record<SourceField, string>> {
  const found: Partial<Record<SourceField, string>> = {};
  if (!DNS_LABEL.test(draft.name)) found.name = "portals.sources.fault.name";
  else if (taken.includes(draft.name)) found.name = "portals.sources.fault.nameTaken";
  if (!draft.type) found.type = "portals.sources.fault.type";
  if (draft.schedule.trim() && !CRON.test(draft.schedule.trim())) found.schedule = "portals.sources.fault.schedule";
  if (draft.type === "endpoints") {
    if (draft.projects.length === 0) found.projects = "portals.sources.fault.projects";
    return found;
  }
  if (!draft.type) return found;
  if (!draft.url.startsWith("https://") || !hostOf(draft.url)) found.url = "portals.sources.fault.url";
  if (hostsOf(draft.allowedHosts).some((host) => !HOST.test(host))) found.allowedHosts = "portals.sources.fault.allowedHosts";
  if (!/^[1-9]\d*$/.test(draft.records.trim())) found.records = "portals.sources.fault.number";
  if (!/^[1-9]\d*$/.test(draft.bytes.trim())) found.bytes = "portals.sources.fault.number";
  return found;
}

/** The `spec.sources[]` entry of a draft the form found no fault in. */
export function sourceOf(draft: SourceDraft): PortalSourceSpec {
  const type = draft.type as SourceType;
  const source: PortalSourceSpec = { name: draft.name, type };
  if (draft.schedule.trim()) source.schedule = draft.schedule.trim();
  if (type === "endpoints") {
    source.projects = [...draft.projects];
    return source;
  }
  source.url = draft.url.trim();
  const own = hostOf(source.url);
  const hosts = hostsOf(draft.allowedHosts);
  // The url's own host is always one the run fetches from (docs/kinds/Portal.md: allowedHosts).
  source.allowedHosts = own && !hosts.includes(own) ? [own, ...hosts] : hosts;
  source.budget = { records: Number(draft.records.trim()), bytes: Number(draft.bytes.trim()) };
  return source;
}

/** The field a server refusal line (`spec.sources[2].allowedHosts: …`) belongs to, for the source at `index`. */
export function sourceFieldOfRefusal(line: string, index: number): SourceField | undefined {
  const member = line.split(":", 1)[0]?.trim() ?? "";
  const prefix = `spec.sources[${index}].`;
  if (!member.startsWith(prefix)) return undefined;
  const rest = member.slice(prefix.length).split(/[.[]/, 1)[0];
  if (rest === "budget") return "records";
  return (["name", "type", "url", "schedule", "allowedHosts", "projects"] as const).find((field) => field === rest);
}

/** The lines of a refusal: each finding of a 422, else its detail. */
function linesOf(error: unknown, generic: string): string[] {
  if (!(error instanceof ApiError)) return [generic];
  return error.problem?.errors?.length ? error.problem.errors : [error.problem?.detail ?? error.message];
}

/** Proposes the Portal `name` with `next` as its spec, a Change for an approver. */
function usePortalChange(name: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (spec: PortalSpec) => {
      const body = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Portal",
        metadata: { name, namespace: ORG_NAMESPACE },
        spec,
      } as unknown as ResourceProposal;
      return proposeChecked(ORG_NAMESPACE, "portals", body, false);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.resource(ORG_NAMESPACE, "portals", name) });
    },
  });
}

/**
 * The Sources tab of a portal (ODP-15, T-6876): its sources with test connection, run now and the
 * last runs, and a form adding one by type. Adding or removing a source proposes a Change.
 */
export function PortalSourcesTab({ name, spec }: { name: string; spec: PortalSpec }): JSX.Element {
  const { t } = useTranslation();
  const sources = (spec.sources ?? []) as PortalSourceSpec[];
  const propose = usePortalChange(name);
  const [change, setChange] = useState<Change | null>(null);
  const [adding, setAdding] = useState(false);
  const [refusal, setRefusal] = useState<string[]>([]);
  const send = (next: PortalSourceSpec[], after?: () => void) => {
    setRefusal([]);
    propose.mutate(
      { ...spec, sources: next },
      {
        onSuccess: (result) => {
          if (isChange(result)) setChange(result);
          after?.();
        },
        onError: (error) => setRefusal(linesOf(error, t("app.error.generic"))),
      },
    );
  };
  return (
    <div className="space-y-4">
      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}
      {refusal.length > 0 && !adding ? (
        <Alert tone="danger" role="alert" title={t("portals.sources.refused")}>
          {refusal.join("; ")}
        </Alert>
      ) : null}
      {sources.length === 0 && !adding ? (
        <EmptyState
          icon="globe"
          title={t("portals.sources.empty")}
          description={t("portals.sources.emptyHint")}
          action={
            <Button variant="primary" onClick={() => setAdding(true)}>
              {t("portals.sources.add")}
            </Button>
          }
        />
      ) : null}
      {sources.map((source) => (
        <SourceCard
          key={source.name}
          portal={name}
          source={source}
          removing={propose.isPending}
          onRemove={() => send(sources.filter((s) => s.name !== source.name))}
        />
      ))}
      {adding ? (
        <AddSourceForm
          organization={spec.organizationRef}
          taken={sources.map((s) => s.name)}
          index={sources.length}
          pending={propose.isPending}
          refusal={refusal}
          onCancel={() => {
            setAdding(false);
            setRefusal([]);
          }}
          onAdd={(source) => send([...sources, source], () => setAdding(false))}
        />
      ) : sources.length > 0 ? (
        <Button variant="secondary" onClick={() => setAdding(true)}>
          {t("portals.sources.add")}
        </Button>
      ) : null}
    </div>
  );
}

function SourceCard({
  portal,
  source,
  removing,
  onRemove,
}: {
  portal: string;
  source: PortalSourceSpec;
  removing: boolean;
  onRemove: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [showRuns, setShowRuns] = useState(false);
  const [notice, setNotice] = useState<{ tone: "success" | "danger" | "info"; text: string } | null>(null);
  const harvested = source.type !== "endpoints";
  const path = { name: portal, source: source.name };
  const queryClient = useQueryClient();
  const runsKey = ["portalSourceRuns", portal, source.name];
  const test = useMutation({
    mutationFn: async () =>
      unwrap(await api.POST("/api/v1/projects/org/portals/{name}/sources/{source}/test", { params: { path } })),
    onSuccess: (answer) => {
      const outcome = answer.outcome;
      setNotice({ tone: outcome === "ok" ? "success" : "danger", text: t(outcomeKey(outcome), { type: source.type }) });
    },
    onError: (error) => setNotice({ tone: "danger", text: refusalText(error, t) }),
  });
  const run = useMutation({
    mutationFn: async () => {
      const answer = await api.POST("/api/v1/projects/org/portals/{name}/sources/{source}/runs", { params: { path } });
      if (answer.error) throw new ApiError(answer.response.status, answer.error.detail ?? answer.response.statusText, answer.error);
    },
    onSuccess: () => {
      setNotice({ tone: "info", text: t("portals.sources.started") });
      setShowRuns(true);
      void queryClient.invalidateQueries({ queryKey: runsKey });
    },
    onError: (error) => setNotice({ tone: "danger", text: refusalText(error, t) }),
  });
  return (
    <Card className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0 space-y-1">
          <h2 className="flex flex-wrap items-center gap-2 text-body font-semibold text-fg">
            <span className="break-all">{source.name}</span>
            <Badge tone="neutral">{t(`portals.sources.type.${source.type}`)}</Badge>
          </h2>
          <p className="break-all text-caption text-fg-muted">
            {harvested ? source.url : t("portals.sources.projectsLine", { projects: (source.projects ?? []).join(", ") || "—" })}
          </p>
          <p className="text-caption text-fg-muted">
            {!harvested
              ? t("portals.sources.endpointsNote")
              : source.schedule
                ? t("portals.sources.scheduleLine", { schedule: source.schedule })
                : t("portals.sources.noSchedule")}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          {harvested ? (
            <>
              <Button size="sm" variant="secondary" aria-label={t("portals.sources.testNamed", { name: source.name })} loading={test.isPending} onClick={() => test.mutate()}>
                {t("portals.sources.test")}
              </Button>
              <Button size="sm" variant="secondary" aria-label={t("portals.sources.runNamed", { name: source.name })} loading={run.isPending} onClick={() => run.mutate()}>
                {t("portals.sources.runNow")}
              </Button>
              <Button size="sm" variant="ghost" aria-expanded={showRuns} onClick={() => setShowRuns(!showRuns)}>
                {showRuns ? t("portals.sources.hideRuns") : t("portals.sources.showRuns")}
              </Button>
            </>
          ) : null}
          <Button size="sm" variant="secondary" className="text-danger" aria-label={t("portals.sources.removeNamed", { name: source.name })} loading={removing} onClick={onRemove}>
            {t("portals.sources.remove")}
          </Button>
        </div>
      </div>
      {notice ? (
        <Alert tone={notice.tone} role={notice.tone === "danger" ? "alert" : "status"}>
          {notice.text}
        </Alert>
      ) : null}
      {showRuns ? <RunLog queryKey={runsKey} portal={portal} source={source.name} /> : null}
    </Card>
  );
}

/** A refusal of a sources route in plain words: the limit, a busy run, no catalogue, or the server's detail. */
function refusalText(error: unknown, t: (key: string, options?: Record<string, unknown>) => string): string {
  if (!(error instanceof ApiError)) return t("app.error.generic");
  if (error.status === 429) return t("portals.sources.limited", { detail: error.problem?.detail ?? "" });
  if (error.status === 409) return t("portals.sources.busy");
  if (error.status === 503) return t("portals.sources.noCatalogue");
  if (error.status === 404) return t("portals.sources.gone");
  return error.problem?.detail ?? error.message;
}

interface RunRow {
  id: string;
  state: "running" | "succeeded" | "failed";
  startedAt?: string;
  finishedAt?: string | null;
  created?: number;
  updated?: number;
  unchanged?: number;
  withdrawn?: number;
  skipped?: number;
  error?: string | null;
}

const STATE_TONE: Record<RunRow["state"], BadgeTone> = { running: "info", succeeded: "success", failed: "danger" };
/** The locale key of each run state (`failed` reads as stopped: the reason column says why). */
const STATE_WORD: Record<RunRow["state"], string> = { running: "running", succeeded: "done", failed: "stopped" };

/** The harvest run log of one source: its last runs, newest first, with counts and the error word. */
function RunLog({ queryKey, portal, source }: { queryKey: string[]; portal: string; source: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const runs = useQuery({
    queryKey,
    retry: false,
    refetchInterval: (query) => ((query.state.data?.items as RunRow[] | undefined)?.some((r) => r.state === "running") ? 5000 : false),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/org/portals/{name}/sources/{source}/runs", { params: { path: { name: portal, source } } })),
  });
  if (runs.isPending) return <PageLoading label={t("app.loading")} />;
  if (runs.isError) return <PageFailed error={runs.error} onRetry={() => void runs.refetch()} />;
  const items = runs.data.items as unknown as RunRow[];
  if (items.length === 0) return <p className="text-caption text-fg-muted">{t("portals.runs.empty")}</p>;
  const when = (at: string | null | undefined) => (at ? new Date(at).toLocaleString(i18n.language) : "—");
  const count = (r: RunRow) =>
    t("portals.runs.countLine", {
      created: r.created ?? 0,
      updated: r.updated ?? 0,
      unchanged: r.unchanged ?? 0,
      withdrawn: r.withdrawn ?? 0,
      skipped: r.skipped ?? 0,
    });
  const state = (r: RunRow) => t(`portals.runs.stateName.${STATE_WORD[r.state] ?? "stopped"}`);
  const reason = (r: RunRow) => (r.error ? t(outcomeKey(r.error), { type: "" }) : "—");
  return (
    <ManifestGrid<RunRow>
      caption={t("portals.runs.caption", { source })}
      storageKey="portal-source-runs"
      searchLabel={t("grid.searchIn", { list: t("portals.runs.caption", { source }) })}
      rows={items}
      rowKey={(r) => r.id}
      query={{ isPending: false, isError: false, error: null, refetch: () => undefined }}
      name={{ id: "started", label: t("portals.runs.started"), text: (r) => when(r.startedAt) }}
      columns={[
        { id: "state", label: t("portals.runs.state"), filter: true, text: state, cell: (r) => <Badge tone={STATE_TONE[r.state] ?? "neutral"}>{state(r)}</Badge> },
        { id: "counts", label: t("portals.runs.counts"), text: count },
        ...(items.some((r) => r.error) ? [{ id: "reason", label: t("portals.runs.reason"), text: reason }] : []),
      ]}
      empty={<p className="text-caption text-fg-muted">{t("portals.runs.empty")}</p>}
    />
  );
}

function AddSourceForm({
  organization,
  taken,
  index,
  pending,
  refusal,
  onCancel,
  onAdd,
}: {
  organization: string;
  taken: string[];
  index: number;
  pending: boolean;
  refusal: string[];
  onCancel: () => void;
  onAdd: (source: PortalSourceSpec) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<SourceDraft>(EMPTY_SOURCE);
  const [shown, setShown] = useState<Partial<Record<SourceField, string>>>({});
  const organizations = useOrganizations();
  // The projects of the portal's Organization the person reads; the judge checks each again (ODP-12).
  const projects = [...(organizations.data?.ownerOf ?? new Map<string, string | undefined>())]
    .filter(([, owner]) => (owner ?? organization) === organization)
    .map(([project]) => project)
    .sort();
  const set = (next: Partial<SourceDraft>) => setDraft({ ...draft, ...next });
  const server: Partial<Record<SourceField, string>> = {};
  const form: string[] = [];
  for (const line of refusal) {
    const field = sourceFieldOfRefusal(line, index);
    if (field && !server[field]) server[field] = line.slice(line.indexOf(":") + 1).trim();
    else form.push(line);
  }
  const errorOf = (field: SourceField): string[] | undefined => {
    const own = shown[field] ? t(shown[field]) : server[field];
    return own ? [own] : undefined;
  };
  const harvested = draft.type !== "" && draft.type !== "endpoints";
  return (
    <form
      noValidate
      aria-labelledby="source-form-title"
      className="max-w-3xl space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        if (pending) return;
        const found = sourceFaults(draft, taken);
        setShown(found);
        const first = (Object.keys(found) as SourceField[])[0];
        if (first) {
          document.getElementById(`source-${first}`)?.focus();
          return;
        }
        onAdd(sourceOf(draft));
      }}
    >
      <Card className="space-y-4">
        <h2 id="source-form-title" className="text-body font-semibold text-fg">
          {t("portals.sources.addTitle")}
        </h2>
        {form.length > 0 ? (
          <Alert tone="danger" role="alert" title={t("portals.sources.refused")}>
            {form.join("; ")}
          </Alert>
        ) : null}
        <Field id="source-type" label={t("portals.sources.field.type")} description={t("portals.sources.hint.type")} required errors={errorOf("type")}>
          <Select id="source-type" value={draft.type} onChange={(event) => set({ type: event.target.value as SourceType | "" })}>
            <option value="">{t("portals.sources.field.typePick")}</option>
            {SOURCE_TYPES.map((type) => (
              <option key={type} value={type}>
                {t(`portals.sources.type.${type}`)}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="source-name" label={t("portals.sources.field.name")} description={t("portals.sources.hint.name")} required errors={errorOf("name")}>
          <Input id="source-name" value={draft.name} autoComplete="off" spellCheck={false} placeholder="kosice" onChange={(event) => set({ name: event.target.value.trim().toLowerCase() })} />
        </Field>
        {draft.type === "endpoints" ? (
          <fieldset className="space-y-2" aria-describedby="source-projects-hint">
            <legend className="text-body font-medium text-fg">{t("portals.sources.field.projects")}</legend>
            <p id="source-projects-hint" className="text-caption text-fg-muted">
              {t("portals.sources.hint.projects")}
            </p>
            {organizations.isPending ? <PageLoading label={t("app.loading")} /> : null}
            {organizations.isSuccess && projects.length === 0 ? (
              <p className="text-caption text-fg-muted">{t("portals.sources.noProjects", { organization })}</p>
            ) : null}
            <div id="source-projects" className="flex flex-wrap gap-4">
              {projects.map((project) => (
                <Checkbox
                  key={project}
                  label={project}
                  checked={draft.projects.includes(project)}
                  onChange={(event) =>
                    set({ projects: event.target.checked ? [...draft.projects, project] : draft.projects.filter((p) => p !== project) })
                  }
                />
              ))}
            </div>
            {errorOf("projects") ? <InlineError>{errorOf("projects")?.[0]}</InlineError> : null}
          </fieldset>
        ) : null}
        {harvested ? (
          <>
            <Field id="source-url" label={t("portals.sources.field.url")} description={t(`portals.sources.hint.url.${draft.type}`)} required errors={errorOf("url")}>
              <Input id="source-url" type="url" value={draft.url} spellCheck={false} placeholder="https://data.example.org/catalog.ttl" onChange={(event) => set({ url: event.target.value })} />
            </Field>
            <Field id="source-allowedHosts" label={t("portals.sources.field.allowedHosts")} description={t("portals.sources.hint.allowedHosts")} errors={errorOf("allowedHosts")}>
              <Input id="source-allowedHosts" value={draft.allowedHosts} spellCheck={false} placeholder="files.example.org" onChange={(event) => set({ allowedHosts: event.target.value })} />
            </Field>
            <div className="grid gap-4 sm:grid-cols-2">
              <Field id="source-records" label={t("portals.sources.field.records")} description={t("portals.sources.hint.records")} required errors={errorOf("records")}>
                <Input id="source-records" inputMode="numeric" value={draft.records} onChange={(event) => set({ records: event.target.value })} />
              </Field>
              <Field id="source-bytes" label={t("portals.sources.field.bytes")} description={t("portals.sources.hint.bytes")} required errors={errorOf("bytes")}>
                <Input id="source-bytes" inputMode="numeric" value={draft.bytes} onChange={(event) => set({ bytes: event.target.value })} />
              </Field>
            </div>
          </>
        ) : null}
        {draft.type ? (
          <Field id="source-schedule" label={t("portals.sources.field.schedule")} description={t("portals.sources.hint.schedule")} errors={errorOf("schedule")}>
            <Input id="source-schedule" value={draft.schedule} spellCheck={false} onChange={(event) => set({ schedule: event.target.value })} />
          </Field>
        ) : null}
      </Card>
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="primary" loading={pending}>
          {t("portals.sources.propose")}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          {t("portals.sources.cancel")}
        </Button>
      </div>
    </form>
  );
}

/** Datasets read per request, the route's own cap, and the most the tab reads. */
const PAGE = 200;
// ponytail: a portal past 2000 datasets lists its first 2000 here; upgrade path: the grid's server pages.
const MOST = 2000;

interface DatasetRow {
  name: string;
  title: string;
  source: string;
  hidden: boolean;
}

/**
 * The Datasets tab of a portal (ODP-15, T-6876): its catalogue's datasets with the source of each,
 * hidden or listed; hiding or listing one again proposes a Change of `spec.hidden`.
 */
export function PortalDatasetsTab({ name, spec, onSources }: { name: string; spec: PortalSpec; onSources: () => void }): JSX.Element {
  const { t } = useTranslation();
  const [change, setChange] = useState<Change | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const propose = usePortalChange(name);
  const datasets = useQuery({
    queryKey: ["portalDatasets", name],
    retry: false,
    queryFn: async () => {
      const read = async (offset: number) =>
        unwrap(await api.GET("/api/v1/projects/org/portals/{name}/datasets", { params: { path: { name }, query: { offset, limit: PAGE } } }));
      const first = await read(0);
      const items = [...first.items];
      while (items.length < Math.min(first.total, MOST)) {
        const next = await read(items.length);
        if (next.items.length === 0) break;
        items.push(...next.items);
      }
      return { total: first.total, items };
    },
  });
  if (datasets.isPending) return <PageLoading label={t("app.loading")} />;
  if (datasets.isError) {
    return datasets.error instanceof ApiError && datasets.error.status === 503 ? (
      <EmptyState icon="globe" title={t("portals.datasets.noCatalogueTitle")} description={t("portals.datasets.noCatalogue")} />
    ) : (
      <PageFailed error={datasets.error} onRetry={() => void datasets.refetch()} />
    );
  }
  const items = datasets.data.items as DatasetRow[];
  const hidden = spec.hidden ?? [];
  const sourceOf = (d: DatasetRow) => (d.source === "endpoints" ? t("portals.sources.type.endpoints") : d.source);
  const toggle = (dataset: string) => {
    setRefusal(null);
    const next = hidden.includes(dataset) ? hidden.filter((h) => h !== dataset) : [...hidden, dataset];
    propose.mutate(
      { ...spec, hidden: next },
      {
        onSuccess: (result) => {
          if (isChange(result)) setChange(result);
        },
        onError: (error) => setRefusal(linesOf(error, t("app.error.generic")).join("; ")),
      },
    );
  };
  if (datasets.data.total === 0) {
    return (
      <EmptyState
        icon="inbox"
        title={t("portals.datasets.empty")}
        description={t("portals.datasets.emptyHint")}
        action={
          <Button variant="primary" onClick={onSources}>
            {t("portals.datasets.toSources")}
          </Button>
        }
      />
    );
  }
  return (
    <div className="space-y-4">
      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}
      {refusal ? (
        <Alert tone="danger" role="alert" title={t("portals.sources.refused")}>
          {refusal}
        </Alert>
      ) : null}
      <p className="text-caption text-fg-muted">{t("portals.datasets.hint")}</p>
      <ManifestGrid<DatasetRow>
        caption={t("portals.datasets.caption")}
        storageKey="portal-datasets"
        searchLabel={t("grid.searchIn", { list: t("portals.datasets.caption") })}
        rows={items}
        rowKey={(d) => d.name}
        total={datasets.data.total}
        query={{ isPending: false, isError: false, error: null, refetch: () => void datasets.refetch() }}
        name={{
          id: "title",
          label: t("portals.datasets.title"),
          text: (d) => `${d.title || d.name} ${d.name}`,
          cell: (d) => (
            <span>
              <span className="font-medium">{d.title || d.name}</span>
              <span className="block break-all text-caption text-fg-muted">{d.name}</span>
            </span>
          ),
        }}
        columns={[
          { id: "source", label: t("portals.datasets.source"), filter: true, text: sourceOf },
          {
            id: "listed",
            label: t("portals.datasets.listed"),
            filter: true,
            text: (d) => (hidden.includes(d.name) ? t("portals.datasets.hidden") : t("portals.datasets.shown")),
            cell: (d) => (
              <Badge tone={hidden.includes(d.name) ? "neutral" : "success"}>{hidden.includes(d.name) ? t("portals.datasets.hidden") : t("portals.datasets.shown")}</Badge>
            ),
          },
        ]}
        actions={(d) => {
          const isHidden = hidden.includes(d.name);
          return (
            <Button
              variant="secondary"
              className="min-h-11 sm:min-h-0"
              loading={propose.isPending && propose.variables?.hidden?.includes(d.name) !== isHidden}
              aria-label={t(isHidden ? "portals.datasets.unhideNamed" : "portals.datasets.hideNamed", { name: d.title || d.name })}
              onClick={() => toggle(d.name)}
            >
              {isHidden ? t("portals.datasets.unhide") : t("portals.datasets.hide")}
            </Button>
          );
        }}
        empty={<p className="text-caption text-fg-muted">{t("portals.datasets.empty")}</p>}
      />
    </div>
  );
}
