import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ListFailed } from "../../components/forms/widgets/ListFailed";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, reasonOf } from "../../api/client";
import { prune } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { useProposal } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { useCreateForm } from "../../components/forms/FormRoute";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { SYNC_ORIGINS, syncSourceSchema } from "../../schemas/kinds";
import type { SyncOriginKind } from "../../schemas/kinds";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { ResourceRowActions } from "../../components/ResourceRowActions";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import type { components } from "../../api/schema";
import {
  Alert,
  Button,
  ConfirmDialog,
  EmptyState,
  ExternalLink,
  Field,
  PageFailed,
  PageHeader,
  PageLoading,
  Select,
  recordCard,
} from "../../components/ui";
import { RecordLink } from "../../components/RecordLink";

export function syncStatusKey(project: string, name: string) {
  return ["projects", project, "syncsources", name, "status"] as const;
}

/** What the manifest says about where a source reads from (MF-27). */
interface SyncSourceSpec {
  source?: {
    git?: { url?: string; ref?: string; path?: string };
    bundle?: { url?: string };
    platformApi?: { baseUrl?: string; project?: string };
  };
  schedule?: { interval?: string; webhook?: boolean };
  mode?: string;
  prune?: boolean;
  autoMerge?: boolean;
}

/** What the Add source dialog holds: a name, one origin, and how the run behaves (MF-27, MF-28). */
interface SyncSourceForm {
  name: string;
  title?: string;
  git?: { url: string; ref: string; path?: string; secretRef?: { name: string; key: string } };
  bundle?: { url: string; secretRef?: { name: string; key: string } };
  platformApi?: { baseUrl: string; project: string; secretRef?: { name: string; key: string } };
  interval: string;
  mode: string;
  conflictPolicy: string;
  prune?: boolean;
  autoMerge?: boolean;
}

/** What a new source starts from: a mirror every six hours that stops at the first conflict. */
const EMPTY_FORM: SyncSourceForm = { name: "", interval: "6h", mode: "mirror", conflictPolicy: "fail" };

export function syncSourceToManifest(project: string, form: SyncSourceForm) {
  const { name, title, git, bundle, platformApi, interval, mode, conflictPolicy, ...rest } = form;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "SyncSource",
    metadata: { name, namespace: project, ...(title?.trim() ? { title } : {}) },
    spec: prune({
      source: { git, bundle, platformApi },
      schedule: { interval },
      mode,
      conflictPolicy,
      ...rest,
    }),
  };
}

/** The form a manifest read back as YAML fills, so the dialog's two views stay one resource. */
export function syncSourceFromManifest(document: unknown): SyncSourceForm {
  const manifest = (document ?? {}) as { metadata?: { name?: string; title?: string }; spec?: SyncSourceSpec };
  const spec = manifest.spec ?? {};
  return {
    name: manifest.metadata?.name ?? "",
    // Every field the form writes comes back, or a switch between the views changes the red lane (F-UI3-018).
    ...(manifest.metadata?.title ? { title: manifest.metadata.title } : {}),
    git: spec.source?.git as SyncSourceForm["git"],
    bundle: spec.source?.bundle as SyncSourceForm["bundle"],
    platformApi: spec.source?.platformApi as SyncSourceForm["platformApi"],
    interval: spec.schedule?.interval ?? "6h",
    mode: spec.mode ?? "mirror",
    conflictPolicy: (spec as { conflictPolicy?: string }).conflictPolicy ?? "fail",
    ...(spec.prune !== undefined ? { prune: spec.prune } : {}),
    ...(spec.autoMerge !== undefined ? { autoMerge: spec.autoMerge } : {}),
  };
}

/**
 * The sync sources of one project: where each reads from, where it stands, and the three
 * things an operator can do to it (MF-27…MF-32, Architecture/06 section 6).
 *
 * The three buttons are three different kinds of act, and the page says so rather than
 * lining them up as equals. **Sync now** runs the loop once. **Pause** switches it off and
 * changes nothing in Git. **Detach** removes the source, which is a change to the repository
 * and therefore a merge request somebody has to approve — the resources the source brought in
 * stay where they are.
 */
export function SyncSourcesPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const list = useQuery({
    queryKey: queryKeys.list(project, "syncsources"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "syncsources" } },
        }),
      ),
  });

  const items = list.data?.items ?? [];

  // A source is added here or nowhere: MF-27 has no other door into the Portal (T-0790).
  const [origin, setOrigin] = useState<SyncOriginKind>("git");
  // `/syncsources/new` opens the form too: the assistant's hand-off and a shared link (T-2582).
  const [adding, setAdding] = useCreateForm();
  const [change, setChange] = useState<Change | null>(null);
  // Held here, as every kind's page holds its form: the Check and the shared draft read what is
  // typed, and a closed form starts over (T-2731, PF-57).
  const [form, setForm] = useState<SyncSourceForm>(EMPTY_FORM);
  const proposal = useProposal(project, "syncsources", (proposed) => {
    setChange(proposed);
    setAdding(false);
    setForm(EMPTY_FORM);
  });

  return (
    <section aria-label={t("syncSources.title")} className="space-y-6">
      <PageHeader
        compact
        title={t("syncSources.title")}
        description={t("syncSources.intro")}
        actions={
          <div className="flex items-end gap-2">
            {/* The shared Field, not a hand-made label: it ties the label to the control and
                carries whatever a message later needs to hang off (UI-01, UI-04). */}
            <Field id="sync-origin-kind" label={t("syncSources.origin")}>
              <Select
                id="sync-origin-kind"
                value={origin}
                onChange={(event) => setOrigin(event.target.value as SyncOriginKind)}
              >
                {SYNC_ORIGINS.map((kind) => (
                  <option key={kind} value={kind}>
                    {t(`syncSources.originKind.${kind}`)}
                  </option>
                ))}
              </Select>
            </Field>
            {/* Each button asks for the verb its route checks (UI-44): a viewer reads why it is
                closed instead of meeting a 403 after filling the form. */}
            <PermissionGuard project={project} kind="SyncSource" verb="propose">
              <Button size="sm" variant="primary" onClick={() => setAdding(true)}>
                {t("syncSources.add")}
              </Button>
            </PermissionGuard>
          </div>
        }
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <ResourceFormDialog<SyncSourceForm>
        kind="SyncSource"
        open={adding}
        onOpenChange={(open) => {
          if (!open) {
            setAdding(false);
            proposal.reset();
            setForm(EMPTY_FORM);
          }
        }}
        project={project}
        draftKind="SyncSource"
        plural="syncsources"
        title={t("syncSources.dialog.title")}
        description={t("syncSources.dialog.description")}
        schema={syncSourceSchema(t, origin)}
        formData={form}
        onChange={(next) => setForm(next ?? EMPTY_FORM)}
        submitLabel={t("syncSources.propose")}
        submitting={proposal.mutation.isPending}
        error={proposal.error}
        source={{
          toManifest: (form) => syncSourceToManifest(project, form),
          fromManifest: syncSourceFromManifest,
        }}
        onSubmit={(form) =>
          proposal.mutation.mutate({ body: syncSourceToManifest(project, form), create: true })
        }
      />

      {/* A list that could not be read is not "no sources yet": the API's own sentence and a
          retry, and the shape of what is coming while it is on its way (UI-15, UI-44). */}
      {list.isPending ? <PageLoading label={t("app.loading")} /> : null}
      {list.isError ? (
        <PageFailed
          error={list.error}
          onRetry={() => {
            void list.refetch();
          }}
        />
      ) : null}

      {!list.isPending && !list.isError && items.length === 0 ? (
        <EmptyState title={t("syncSources.empty")}
          description={t("syncSources.emptyHint")}
          action={
            <PermissionGuard project={project} kind="SyncSource" verb="propose">
              <Button size="sm" variant="primary" onClick={() => setAdding(true)}>
                {t("syncSources.add")}
              </Button>
            </PermissionGuard>
          } />
      ) : null}

      <ul className="space-y-4">
        {items.map((item) => (
          <li key={item.metadata.name}>
            <SyncSourceCard
              project={project}
              name={item.metadata.name}
              spec={item.spec as SyncSourceSpec | undefined}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}

function SyncSourceCard({
  project,
  name,
  spec,
}: {
  project: string;
  name: string;
  spec: SyncSourceSpec | undefined;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [detached, setDetached] = useState<string | null>(null);
  // Open while the dialog names the source and asks; detaching is not undone by asking again.
  const [detaching, setDetaching] = useState(false);

  const status = useQuery({
    queryKey: syncStatusKey(project, name),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/syncsources/{name}/status", {
          params: { path: { project, name } },
        }),
      ),
  });

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: syncStatusKey(project, name) });
  };

  const syncNow = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/syncsources/{name}/sync", {
          params: { path: { project, name } },
        }),
      ),
    onSuccess: refresh,
  });

  const pause = useMutation({
    mutationFn: async (paused: boolean) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/syncsources/{name}/pause", {
          params: { path: { project, name } },
          body: { paused },
        }),
      ),
    onSuccess: refresh,
  });

  const detach = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/syncsources/{name}/detach", {
          params: { path: { project, name } },
        }),
      ),
    onSuccess: (result) => {
      const url = (result as { mergeRequest?: string } | undefined)?.mergeRequest;
      setDetached(url ?? null);
      refresh();
    },
  });

  const busy = syncNow.isPending || pause.isPending || detach.isPending;
  const failure = [syncNow.error, pause.error, detach.error].find(Boolean);
  const paused = status.data?.paused ?? false;

  return (
    // The page's own H1 is above this list, so a source is an H2: an H3 skipped a level and a
    // screen reader's heading list read every source as belonging to a section that was never
    // there (UI-16, measured by axe's heading-order).
    <article
      aria-labelledby={`sync-source-${name}`}
      onClick={recordCard.onClick}
      onAuxClick={recordCard.onAuxClick}
      className={`rounded border border-border p-4 ${recordCard.className}`}
    >
      <header className="flex flex-wrap items-baseline gap-3">
        <h2 id={`sync-source-${name}`} className="font-mono text-body font-bold">
          <RecordLink project={project} plural="syncsources" name={name} />
        </h2>
        <LifecycleBadge kind="phase" value={status.data?.phase} />
        <span className="text-caption">{scheduleOf(spec, t, i18n.resolvedLanguage ?? i18n.language ?? "en")}</span>
        {/* Sync now, Pause and Detach are what this card is for and stay in its body; editing
            and deleting the source itself are one menu at the end of the header (T-2288). */}
        <span className="ml-auto flex items-center gap-1.5">
          <ResourceRowActions
            project={project}
            target={{ project, kind: "SyncSource", plural: "syncsources", name }}
          />
        </span>
      </header>

      {status.isError ? (
        <ListFailed
          what={t("syncSources.status")}
          reason={reasonOf(status.error, t("form.invalid"))}
          onRetry={refresh}
        />
      ) : null}

      <dl className="mt-3 grid gap-x-6 gap-y-1 text-body sm:grid-cols-[max-content_1fr]">
        <dt className="font-medium">{t("syncSources.origin")}</dt>
        <dd className="break-all font-mono text-caption">{originOf(spec) ?? "—"}</dd>

        <dt className="font-medium">{t("syncSources.revision")}</dt>
        <dd className="font-mono text-caption">
          {status.data?.observedRevision ? status.data.observedRevision.slice(0, 12) : "—"}
        </dd>

        {status.data?.mergeRequest ? (
          <>
            <dt className="font-medium">{t("syncSources.mergeRequest")}</dt>
            <dd>
              <ExternalLink href={status.data.mergeRequest}>
                {t("syncSources.review")}
              </ExternalLink>
            </dd>
          </>
        ) : null}

        {status.data?.lastError ? (
          <>
            <dt className="font-medium">{t("syncSources.lastError")}</dt>
            <dd className="text-danger">{status.data.lastError}</dd>
          </>
        ) : null}
      </dl>

      {status.data && !status.data.durable ? (
        <p className="mt-3 text-caption">{t("syncSources.notDurable")}</p>
      ) : null}

      <RunReport report={syncNow.data} />

      {detached ? (
        <p role="status" className="mt-3 text-body">
          {t("syncSources.detached")}{" "}
          <ExternalLink href={detached}>{t("syncSources.review")}</ExternalLink>
        </p>
      ) : null}

      {failure ? (
        <Alert role="alert" tone="danger" className="mt-3">
          {failure instanceof ApiError
            ? (failure.problem?.detail ?? failure.message)
            : t("app.error.generic")}
        </Alert>
      ) : null}

      <div className="mt-4 flex flex-wrap gap-2">
        <PermissionGuard project={project} kind="SyncSource" verb="propose">
          <Button
            size="sm"
            disabled={busy || paused}
            disabledReason={busy ? t("app.busyReason") : t("syncSources.pausedReason")}
            onClick={() => syncNow.mutate()}
          >
            {t("syncSources.syncNow")}
          </Button>
        </PermissionGuard>
        <PermissionGuard project={project} kind="SyncSource" verb="propose">
          <Button
            size="sm"
            disabled={busy}
            disabledReason={t("app.busyReason")}
            onClick={() => pause.mutate(!paused)}
          >
            {paused ? t("syncSources.resume") : t("syncSources.pause")}
          </Button>
        </PermissionGuard>
        <PermissionGuard project={project} kind="SyncSource" verb="delete">
          <Button
            variant="danger"
            size="sm"
            disabled={busy}
            disabledReason={t("app.busyReason")}
            onClick={() => {
              setDetaching(true);
            }}
          >
            {t("syncSources.detach")}
          </Button>
        </PermissionGuard>
      </div>
      <ConfirmDialog
        open={detaching}
        onOpenChange={setDetaching}
        title={t("syncSources.detach")}
        description={t("syncSources.detachConfirm", { name })}
        confirmLabel={t("syncSources.detach")}
        pending={detach.isPending}
        onConfirm={() => detach.mutate(undefined, { onSettled: () => setDetaching(false) })}
      />
    </article>
  );
}

/** What a forced run did, when one has been asked for. */
function RunReport({ report }: { report: components["schemas"]["SyncRunReport"] | undefined }) {
  const { t } = useTranslation();
  if (!report) {
    return null;
  }
  return (
    <div className="mt-3 space-y-1 text-body">
      {report.proposed.length > 0 ? (
        <p role="status">{t("syncSources.proposed", { count: report.proposed.length })}</p>
      ) : report.flags.length === 0 ? (
        <p role="status">{t("syncSources.nothingToDo")}</p>
      ) : null}
      {report.flags.map((flag) => (
        <Alert key={flag} role="alert" tone="danger" className="mt-1">
          {flag}
        </Alert>
      ))}
    </div>
  );
}

/** The origin in one line, whichever of the three a source names. */
function originOf(spec: SyncSourceSpec | undefined): string | undefined {
  const source = spec?.source;
  if (source?.git?.url) {
    const subtree = source.git.path ? ` (${source.git.path})` : "";
    return `${source.git.url}#${source.git.ref ?? "HEAD"}${subtree}`;
  }
  if (source?.bundle?.url) {
    return source.bundle.url;
  }
  if (source?.platformApi?.baseUrl) {
    return `${source.platformApi.baseUrl}/${source.platformApi.project ?? ""}`;
  }
  return undefined;
}

/** How often it runs, in the words the manifest uses (MF-28). */
function scheduleOf(
  spec: SyncSourceSpec | undefined,
  t: (key: string, options?: Record<string, unknown>) => string,
  language: string,
): string {
  if (spec?.schedule?.webhook) {
    return t("syncSources.byWebhook");
  }
  if (spec?.schedule?.interval) {
    return t("syncSources.everyInterval", { interval: intervalWords(spec.schedule.interval, language) });
  }
  return "";
}

const INTERVAL_UNITS = { s: "second", m: "minute", h: "hour", d: "day", w: "week" } as const;

/** `6h`, `1d`, `30m` as "6 hours", "1 day", "30 minutes" in the reader's language; anything else as written. */
export function intervalWords(raw: string, language: string): string {
  const match = /^(\d+)([smhdw])$/.exec(raw.trim());
  if (!match) return raw;
  const unit = INTERVAL_UNITS[match[2] as keyof typeof INTERVAL_UNITS];
  return new Intl.NumberFormat(language, { style: "unit", unit, unitDisplay: "long" }).format(Number(match[1]));
}
