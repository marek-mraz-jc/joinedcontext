import { AlertDialog } from "../components/AlertDialog";
import { FlowMapView } from "../pages/pipelines/FlowMapView";
import { Tabs, tabPanelProps } from "../components/ui/Tabs";
import { useFormRoute, useOpenFromAddress } from "../components/forms/FormRoute";
import { RecordLink } from "../components/RecordLink";
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ChangeNotice } from "../components/ChangeNotice";
import { TypeLink } from "../pages/models/ModelLinks";
import { ManifestGrid } from "../components/grid/ManifestGrid";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { PipelineEditorDialog, expiryWindow, fromManifest } from "../pages/pipelines/PipelineEditor";
import { KpiEditorDialog } from "../pages/pipelines/KpiEditor";
import { PipelineRejectedDialog } from "../pages/pipelines/PipelineRejected";
import { PipelineRunsDialog } from "../pages/pipelines/PipelineRuns";
import { PipelineHealthView } from "../pages/pipelines/PipelineHealth";
import {
  LastRunCell,
  dataSourceOf,
  proposeEnabled,
  executionClass,
  useLatestRuns,
  useStreamCounters,
} from "../pages/pipelines/PipelineOverview";
import type { PipelineSpec } from "../pages/pipelines/PipelineOverview";
import type { PipelineForm, toEnvelope } from "../pages/pipelines/PipelineEditor";
import { takeEditRequest, takePrefill } from "../assistant/state";
import { useProjectUsage } from "../components/ProjectQuota";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { EditResourceDialog } from "../components/EditResourceDialog";
import { usePermissions } from "../api/permissions";
import { proposeChecked } from "../api/proposal";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
  Input,
  Select,
} from "../components/ui";

/**
 * Why a Live stream lands nothing, from its `StreamWriting` condition, or `null` when it writes
 * (T-2967): the reconciler sets it when the runner counts errors and nothing sent, or records
 * taken in for ten minutes with nothing sent, rejected or failed.
 */
export function notWriting(pipeline: Manifest): string | null {
  const condition = (pipeline.status?.conditions ?? []).find(
    (candidate) => candidate.type === "StreamWriting" && candidate.status === "False",
  );
  return condition ? (condition.message ?? condition.reason ?? "") : null;
}

export { executionClass };

/** When one row's pipeline last ran, with a resident runner's totals beside it (T-4817). */
function RowLastRun({
  project,
  name,
  latest,
  counted,
  now,
}: {
  project: string;
  name: string;
  /** Every pipeline's latest run, read once for the whole list (T-5818). */
  latest: ReturnType<typeof useLatestRuns>;
  /** A running resident stream: its runner has counters to read. */
  counted: boolean;
  now: number;
}): JSX.Element {
  const counters = useStreamCounters(project, name, counted);
  return (
    <LastRunCell
      project={project}
      name={name}
      run={latest.data ? (latest.data.get(name) ?? null) : undefined}
      failed={latest.isError}
      // "4 min ago" moves on with each refetch rather than staying where the page opened.
      now={Math.max(now, latest.dataUpdatedAt)}
      counters={counted ? counters.data : undefined}
    />
  );
}

type StatusFilter = "all" | "running" | "paused" | "notWriting";
type ClassFilter = "all" | "resident" | "scheduled";

/** Whether a pipeline passes the list's search and filters (T-4817). */
export function pipelineMatches(
  pipeline: Manifest,
  needle: string,
  status: StatusFilter,
  klass: ClassFilter,
  locale: string,
): boolean {
  const spec = pipeline.spec as PipelineSpec;
  const running = spec.enabled !== false;
  if (status === "running" && !running) return false;
  if (status === "paused" && running) return false;
  if (status === "notWriting" && notWriting(pipeline) === null) return false;
  if (klass !== "all" && executionClass(spec) !== klass) return false;
  const words = needle.trim().toLowerCase();
  if (words === "") return true;
  return [
    pipeline.metadata.name,
    localized(pipeline.metadata.title, locale, ""),
    dataSourceOf(spec) ?? "",
    spec.output?.type ?? "",
  ].some((text) => text.toLowerCase().includes(words));
}

/** Ingestion pipelines of one project: what runs where, how it is doing, and its credentials. */
type PipelineView = "list" | "map" | "health";

/**
 * The create form's values a hand-off carries: the form itself, or a Pipeline manifest the
 * assistant's open_form built (T-5959), read back into the form.
 */
export function prefilledForm(taken: Record<string, unknown> | null): PipelineForm | undefined {
  if (!taken) {
    return undefined;
  }
  if (taken.kind === "Pipeline" && typeof taken.metadata === "object" && taken.metadata !== null) {
    try {
      return fromManifest(taken);
    } catch {
      return undefined;
    }
  }
  return taken as PipelineForm;
}

function viewFromAddress(): PipelineView {
  const view = typeof window === "undefined" ? null : new URLSearchParams(window.location.search).get("view");
  return view === "map" || view === "health" ? view : "list";
}

function rememberView(view: PipelineView): void {
  const url = new URL(window.location.href);
  if (view !== "list") {
    url.searchParams.set("view", view);
  } else {
    url.searchParams.delete("view");
    url.searchParams.delete("space");
  }
  window.history.replaceState(window.history.state, "", url);
}

export function PipelinesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const usage = useProjectUsage(project);
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The assistant may have sent the person here with a form in hand (UI-45), or with a change
  // to one pipeline (`?edit=`, AG-77): taken once, before the first render, so the editor is
  // open from the start and a reload starts clean.
  const [request, setRequest] = useState(() => takeEditRequest());
  // A draft the assistant or another window left (`?draft=`, AG-61): the editor opens on it.
  const [urlDraftName] = useState(() =>
    typeof window === "undefined"
      ? undefined
      : (new URLSearchParams(window.location.search).get("draft") ?? undefined),
  );
  const [editing, setEditing] = useState<Manifest | null>(() => (request?.manifest as Manifest | null) ?? null);
  const [initial, setInitial] = useState(() => (request ? undefined : prefilledForm(takePrefill(window.location.pathname))));
  // The list or the data flow map (T-4862), kept in the address so a link opens the map.
  const [view, setView] = useState<PipelineView>(viewFromAddress);
  const [dialogOpen, setDialogOpen] = useState(
    initial !== undefined || editing !== null || urlDraftName !== undefined,
  );
  const [formError, setFormError] = useState<string | null>(null);
  // The KPI editor over several sources (PL-105), opened from the KPI recipe.
  const [kpiOpen, setKpiOpen] = useState(false);
  // The pipeline whose rejected records are open (PL-61).
  const [rejectedOf, setRejectedOf] = useState<string | null>(null);
  // The pipeline whose runs and log are open (PL-62).
  const [runsOf, setRunsOf] = useState<string | null>(null);
  // The run a rejected record names, opened when the person follows it (T-3252).
  const [runOpened, setRunOpened] = useState<string | null>(null);
  // The pipeline whose alerts are being chosen (T-3261).
  const [alertOf, setAlertOf] = useState<string | null>(null);
  const [needle, setNeedle] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [classFilter, setClassFilter] = useState<ClassFilter>("all");
  // One clock per page view: every row's "4 min ago" is measured from the same instant.
  const [now] = useState(() => Date.now());

  const list = useQuery({
    queryKey: queryKeys.list(project, "pipelines"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "pipelines" } },
        }),
      ),
  });
  const latestRuns = useLatestRuns(project);

  // Pausing is a repository change like any other, never a live switch (CC-35).
  const toggle = useMutation({
    mutationFn: async ({ pipeline, run }: { pipeline: Manifest; run: boolean }) => {
      setError(null);
      return proposeEnabled(project, pipeline, run);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "pipelines") });
    },
    onError: (err) => {
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : t("app.error.generic"),
      );
    },
  });

  /**
   * A create is a POST, an edit a PUT at the manifest's own path; either one lands in the
   * forge and in Approvals like every other write (AP-13). Every signed-in user may propose:
   * the approval, not the button, is the gate.
   */
  const propose = useMutation({
    mutationFn: async ({
      envelope,
      name,
      draft,
      files,
      unchecked,
    }: {
      envelope: ReturnType<typeof toEnvelope>;
      name: string | null;
      /** No check of the manifest yet: the KPI editor checks its fields, not the proposal (PF-57). */
      unchecked?: boolean;
      /** The draft the form edited, so the proposal is taken from what was checked (AG-62). */
      draft?: { kind: string; name: string };
      /** Files beside the manifest, an edited `bento.yaml` (T-4818). */
      files?: Record<string, string>;
    }) => {
      setFormError(null);
      const body = files ? { ...envelope, files } : draft ? { ...envelope, draft } : envelope;
      if (unchecked) return proposeChecked(project, "pipelines", body, true);
      return unwrap(
        name === null
          ? await api.POST("/api/v1/projects/{project}/{plural}", {
              params: { path: { project, plural: "pipelines" } },
              body,
            })
          : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
              params: { path: { project, plural: "pipelines", name } },
              body,
            }),
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      setEditing(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "pipelines") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : t("app.error.generic"),
      );
    },
  });

  // A link to one pipeline's editor without a change in hand opens the stored manifest once the
  // list has it.
  const requested =
    request && !request.manifest
      ? asManifests(list.data?.items ?? []).find((pipeline) => pipeline.metadata.name === request.name)
      : undefined;
  if (requested) {
    setRequest(null);
    openEditor(requested);
  }

  function openEditor(pipeline: Manifest | null) {
    setEditing(pipeline);
    setFormError(null);
    setDialogOpen(true);
  }

  const rowParts = (pipeline: Manifest) => {
    const spec = pipeline.spec as PipelineSpec;
    const klass = executionClass(spec);
    // Absent means running: a pipeline is only paused by an explicit `false`.
    const running = spec.enabled !== false;
    const stalled = notWriting(pipeline);
    const counted = running && klass === "resident";
    const source = dataSourceOf(spec);
    const phase = (
      <>
        {/* The row said "Live" beside a stream that said "Paused" (T-2759): a paused
            pipeline is applied and stopped, and stopped is what a person needs to read. */}
        <LifecycleBadge
          kind="phase"
          value={
            !running && ["live", "applied", "synced"].includes((pipeline.status?.phase ?? "").toLowerCase())
              ? "paused"
              : pipeline.status?.phase
          }
        />
        {/* Live says the stream is deployed; whether anything lands is this condition
            (T-2967): hsl-hfp-vehicles read Live for half an hour of writing nothing. */}
        {stalled !== null ? (
          <div className="mt-1">
            <Badge tone="warning" title={stalled || undefined}>
              {t("pipelines.notWriting")}
              {stalled ? <span className="sr-only">: {stalled}</span> : null}
            </Badge>
          </div>
        ) : null}
      </>
    );
    return { spec, klass, running, counted, phase, source };
  };

  const pipelines = asManifests(list.data?.items ?? []);
  const shown = pipelines.filter((pipeline) =>
    pipelineMatches(pipeline, needle, statusFilter, classFilter, locale),
  );
  // On a routed list the editor is a page at `/pipelines/new` and `/pipelines/{name}/edit`
  // (T-2474): the controls go to the address, and the address opens the editor.
  const formRoute = useFormRoute();
  // A person who may not propose a pipeline still opens one: filled and closed, not the editor
  // whose proposal the server would refuse (T-2875).
  const permissions = usePermissions(project);
  const viewOnly = !permissions.isLoading && !permissions.can("Pipeline", "propose");
  const steward = !permissions.isLoading && permissions.can("Pipeline", "approve");
  useOpenFromAddress(list.data ? pipelines : undefined, (pipeline) => pipeline.metadata.name, {
    create: () => openEditor(null),
    edit: openEditor,
  });
  // The reason before the person types: a project at its resident-pipeline quota refuses the next
  // one, and until T-1594 it said so only after the form was filled and submitted. The numbers are
  // the API's own (PF-73, PF-75); the page only reads them. The control stays enabled, because a
  // scheduled pipeline does not count against this dimension and is still allowed.
  const resident = usage.data?.residentPipelines;
  const residentFull =
    resident?.limit !== undefined && (resident.used ?? 0) >= resident.limit;

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        compact
        title={t("pipelines.title")}
        description={t("pipelines.lead")}
        actions={
          <PermissionGuard project={project} kind="Pipeline" verb="propose">
            <Button
              variant="primary"
              icon={<Icon name="plus" className="size-4" />}
              onClick={() => (formRoute ? formRoute.openNew() : openEditor(null))}
            >
              {t("pipelines.add")}
            </Button>
          </PermissionGuard>
        }
      />

      <Tabs
        id="pipeline-views"
        label={t("pipelines.map.views")}
        tabs={[
          { value: "list", label: t("pipelines.map.view.list") },
          { value: "map", label: t("pipelines.map.view.map") },
          // Each source's health (T-5048) is for stewards: the server refuses anyone else.
          ...(steward
            ? [{ value: "health" as const, label: t("pipelines.map.view.health") }]
            : []),
        ]}
        value={view}
        onChange={(next) => {
          setView(next);
          rememberView(next);
        }}
      />

      {view === "health" && steward ? (
        <div {...tabPanelProps("pipeline-views", "health")}>
          <PipelineHealthView project={project} onOpenRuns={setRunsOf} />
        </div>
      ) : view === "map" ? (
        <div {...tabPanelProps("pipeline-views", "map")}>
          <FlowMapView
            project={project}
            onNewPipeline={(form) => {
              setInitial(form as PipelineForm);
              if (formRoute) {
                formRoute.openNew();
              } else {
                openEditor(null);
              }
            }}
          />
        </div>
      ) : (
      <div {...tabPanelProps("pipeline-views", "list")} className="flex flex-col gap-section">
      {residentFull ? (
        <Alert role="status" tone="warning">
          {t("quota.dimensionExceeded", {
            limit: resident?.limit,
            dimension: t("quota.dimension.residentPipelines"),
          })}
        </Alert>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}

      {pipelines.length > 0 ? (
        <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-end">
          <Input
            type="search"
            aria-label={t("pipelines.filter.find")}
            placeholder={t("pipelines.filter.find")}
            value={needle}
            onChange={(event) => setNeedle(event.target.value)}
            className="sm:max-w-sm"
          />
          <Select
            aria-label={t("pipelines.filter.status")}
            value={statusFilter}
            onChange={(event) => setStatusFilter(event.target.value as StatusFilter)}
            className="sm:w-auto"
          >
            {(["all", "running", "paused", "notWriting"] as const).map((value) => (
              <option key={value} value={value}>
                {t(`pipelines.filter.statusOption.${value}`)}
              </option>
            ))}
          </Select>
          <Select
            aria-label={t("pipelines.filter.class")}
            value={classFilter}
            onChange={(event) => setClassFilter(event.target.value as ClassFilter)}
            className="sm:w-auto"
          >
            {(["all", "resident", "scheduled"] as const).map((value) => (
              <option key={value} value={value}>
                {t(`pipelines.filter.classOption.${value}`)}
              </option>
            ))}
          </Select>
          {shown.length !== pipelines.length ? (
            <p role="status" className="text-caption text-fg-subtle sm:self-center">
              {t("pipelines.filter.shown", { shown: shown.length, total: pipelines.length })}
            </p>
          ) : null}
        </div>
      ) : null}

      {/* An empty list is where a person decides what to do next, so it says what the
          two ways in are rather than only that there is nothing here (UI-01). */}
      <ManifestGrid<Manifest>
        caption={t("pipelines.title")}
        storageKey="pipelines"
        searchLabel={t("grid.searchIn", { list: t("pipelines.title") })}
        search={needle}
        onSearchChange={setNeedle}
        rows={shown}
        rowKey={(row) => row.metadata.name}
        query={list}
        name={{
          id: "name",
          label: t("pipelines.field.name"),
          text: (pipeline) => localized(pipeline.metadata.title, locale, pipeline.metadata.name),
          cell: (pipeline) => {
            const { spec, source } = rowParts(pipeline);
            return (
              <>
                  <div>
                    <RecordLink project={project} plural="pipelines" name={pipeline.metadata.name}>
                      {localized(pipeline.metadata.title, locale, pipeline.metadata.name)}
                    </RecordLink>
                  </div>
                  {pipeline.metadata.title ? (
                    <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                      {pipeline.metadata.name}
                    </div>
                  ) : null}
                  {source ? (
                    <div className="mt-0.5 text-caption text-fg-subtle">
                      {t("pipelines.readsSource")} <span className="font-mono">{source}</span>
                    </div>
                  ) : null}
                  {typeof spec.output?.type === "string" && spec.output.type !== "" ? (
                    <div className="mt-0.5 text-caption text-fg-subtle">
                      {t("pipelines.writesType")}{" "}
                      <TypeLink project={project} type={spec.output.type} className="font-mono" />
                    </div>
                  ) : null}
                  {typeof spec.expiry?.after === "string" ? (
                    <div className="mt-0.5 text-caption text-fg-subtle">
                      {t("pipelines.expiryRemoves", { window: expiryWindow(spec.expiry.after, locale) })}
                    </div>
                  ) : null}
                  {(() => {
                    // The sweep runs as its own stream; one the runner refused leaves the pipeline
                    // writing and nothing removed, which the row says rather than the log (PL-64, T-3091).
                    const sweep = (pipeline.status?.conditions ?? []).find(
                      (condition) => condition.type === "ExpirySweep" && condition.status === "False",
                    );
                    return sweep && typeof spec.expiry?.after === "string" ? (
                      <div className="mt-0.5 text-caption text-danger [overflow-wrap:anywhere]" data-testid="pipeline-expiry-refused">
                        {t("pipelines.expiryRefused", { reason: sweep.message ?? sweep.reason ?? "" })}
                      </div>
                    ) : null;
                  })()}
              </>
            );
          },
        }}
        columns={[
          {
            id: "class",
            label: t("pipelines.field.class"),
            field: "class",
            text: (pipeline) => t(`pipelines.class.${rowParts(pipeline).klass}`),
            cell: (pipeline) => {
              const { klass } = rowParts(pipeline);
              return (
                <Badge tone={klass === "resident" ? "info" : "neutral"} title={t(`pipelines.class.${klass}Help`)}>
                  {t(`pipelines.class.${klass}`)}
                </Badge>
              );
            },
          },
          {
            id: "phase",
            label: t("pipelines.field.phase"),
            field: "enabled",
            text: (pipeline) => pipeline.status?.phase ?? "",
            cell: (pipeline) => rowParts(pipeline).phase,
          },
          {
            id: "lastRun",
            label: t("pipelines.field.lastRun"),
            text: () => "",
            cell: (pipeline) => (
              <RowLastRun project={project} name={pipeline.metadata.name} latest={latestRuns} counted={rowParts(pipeline).counted} now={now} />
            ),
          },
          { id: "period", label: t("grid.field.period"), field: "period", hidden: true, text: (pipeline) => String((pipeline.spec as { period?: string }).period ?? "") },
          { id: "schedule", label: t("grid.field.schedule"), field: "schedule", hidden: true, text: (pipeline) => String((pipeline.spec as { schedule?: string }).schedule ?? "") },
        ]}
        actions={(pipeline) => {
          const { running } = rowParts(pipeline);
          return (
            <div className="flex items-center justify-end gap-1.5">
              <ResourceRowActions
                project={project}
                target={{
                  project,
                  kind: "Pipeline",
                  plural: "pipelines",
                  name: pipeline.metadata.name,
                }}
                extra={[
                  {
                    key: "runs",
                    label: t("pipelines.runs.open"),
                    onSelect: () => setRunsOf(pipeline.metadata.name),
                  },
                  {
                    key: "rejected",
                    label: t("pipelines.rejected.open"),
                    onSelect: () => setRejectedOf(pipeline.metadata.name),
                  },
                  {
                    key: "alerts",
                    label: t("alerts.open"),
                    onSelect: () => setAlertOf(pipeline.metadata.name),
                  },
                ]}
                onEdit={() =>
                  formRoute ? formRoute.openEdit(pipeline.metadata.name) : openEditor(pipeline)
                }
                primary={
                  // A phone card has room for it beside the menu (T-6115 review): Pause stays one tap away.
                  <Button
                    size="sm"
                    className="whitespace-nowrap"
                    disabled={toggle.isPending}
                    onClick={() => toggle.mutate({ pipeline, run: !running })}
                    title={t(running ? "pipelines.pauseHint" : "pipelines.resumeHint")}
                  >
                    {t(running ? "pipelines.pause" : "pipelines.resume")}
                  </Button>
                }
                sourceUrl={pipeline.status?.sourceUrl}
              />
            </div>
          );
        }}
        edit={{ project, plural: "pipelines", kind: "Pipeline", manifest: (row) => row, onChange: setChange }}
        empty={
          pipelines.length > 0 ? (
            <EmptyState
              bare
              icon="pipelines"
              title={t("pipelines.filter.none")}
              description={t("pipelines.filter.noneHint")}
            />
          ) : (
          <EmptyState
            bare
            icon="pipelines"
            title={t("pipelines.empty")}
            description={t("pipelines.emptyHint")}
            action={
              <PermissionGuard project={project} kind="Pipeline" verb="propose">
                <Button variant="primary" onClick={() => (formRoute ? formRoute.openNew() : openEditor(null))}>
                  {t("pipelines.add")}
                </Button>
              </PermissionGuard>
            }
          />
          )
        }
      />
      </div>
      )}

      {rejectedOf !== null ? (
        <PipelineRejectedDialog
          project={project}
          name={rejectedOf}
          onClose={() => setRejectedOf(null)}
          onOpenRun={(run) => {
            setRunsOf(rejectedOf);
            setRunOpened(run);
            setRejectedOf(null);
          }}
        />
      ) : null}
      {alertOf !== null ? (
        <AlertDialog project={project} scope="pipeline" target={alertOf} label={alertOf} onClose={() => setAlertOf(null)} />
      ) : null}
      {runsOf !== null ? (
        <PipelineRunsDialog
          project={project}
          name={runsOf}
          initialRun={runOpened ?? undefined}
          // A pipeline the queue runs says so in its status (PL-78); it alone can run now (T-4349).
          runsFromQueue={pipelines
            .find((pipeline) => pipeline.metadata.name === runsOf)
            ?.status?.conditions?.some((condition) => condition.type === "Scheduled")}
          onClose={() => {
            setRunsOf(null);
            setRunOpened(null);
          }}
        />
      ) : null}

      {dialogOpen && editing && viewOnly ? (
        <EditResourceDialog
          readOnly
          open
          target={{ project, kind: "Pipeline", plural: "pipelines", name: editing.metadata.name }}
          onOpenChange={(open) => {
            if (!open) {
              setDialogOpen(false);
              setEditing(null);
            }
          }}
        />
      ) : dialogOpen ? (
        <PipelineEditorDialog
          project={project}
          onOpenChange={(open) => {
            if (!open) {
              setDialogOpen(false);
              setEditing(null);
              // A prefill opens the editor once; the next New starts empty.
              setInitial(undefined);
            }
          }}
          editing={editing}
          initial={initial}
          pending={propose.isPending}
          error={formError}
          draftName={urlDraftName}
          onSubmit={(envelope, draft, files) =>
            propose.mutate({ envelope, name: editing ? editing.metadata.name : null, draft, files })
          }
          onSeveralSources={() => {
            setDialogOpen(false);
            setInitial(undefined);
            setKpiOpen(true);
          }}
        />
      ) : null}
      {kpiOpen ? (
        <KpiEditorDialog
          project={project}
          pending={propose.isPending}
          error={formError}
          onClose={() => setKpiOpen(false)}
          onPropose={(envelope) => propose.mutate({ envelope, name: null, unchecked: true }, { onSuccess: () => setKpiOpen(false) })}
        />
      ) : null}
    </div>
  );
}
