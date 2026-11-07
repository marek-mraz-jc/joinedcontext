import { ProjectUsageSection } from "../../components/endpoints/UsageSection";
import type { JSX } from "react";
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import type { BadgeTone } from "../../components/ui/Badge";
import {
  Alert,
  Badge,
  EmptyState,
  Field,
  PageFailed,
  PageHeader,
  Select,
  Table,
  TableSkeleton,
  safeHref,
} from "../../components/ui";

type Check = components["schemas"]["HealthCheck"];
type Status = components["schemas"]["HealthCheckStatus"];

/** The checks with a label of their own (`src/health_log.rs`, `src/health_probe.rs`); a surface shows by its name. */
export const CHECKS = ["reconcile", "runs", "freshness", "entities", "storage", "validation", "reachable"] as const;
/** The message keys the server sends (`SAY` in `src/health_log.rs`), each under `health.say`. */
export const SAID = [
  "notApplied", "waitingApproval", "noNewData", "countFellToZero", "countHalved", "counted", "notCounted",
  "invalid", "valid", "staleType", "noReader", "readBy", "readerSaid", "answered", "slow", "anonymousRefused",
  "notServed", "noAnswer", "policyWrite", "policy", "token", "robots", "sourceDown", "serviceDown", "runLog",
  "manifest", "approve", "drift", "drop", "schedule", "countFell", "validation", "retention", "addReader",
  "publicRefused", "surfaceMissing", "slowSurface",
] as const;
const TONE: Record<Status, BadgeTone> = { error: "danger", warn: "warning", ok: "success" };
/** The label of each status; the key is the reader's word, not the server's (ui-writing). */
export const LABEL: Record<Status, string> = { error: "health.broken", warn: "health.look", ok: "health.working" };
/** Height says the status as well as colour does: full for broken, half for a warning, low for working. */
const BAR: Record<Status, string> = { error: "h-full bg-danger", warn: "h-1/2 bg-warning", ok: "h-1/4 bg-success" };

/** A message in the reader's language when the server named its key, else the component's words. */
function useSay(): (text: Check["detail"]) => string {
  const { t } = useTranslation();
  return (text) =>
    text.code === "" || !(SAID as readonly string[]).includes(text.code)
      ? text.text
      : t(`health.say.${text.code}`, { ...text.args, defaultValue: text.text });
}

/** Which checks the list shows: those needing attention (broken or a warning), one status, or all. */
type Shown = "attention" | Status | "all";
export const SHOWN: readonly Shown[] = ["attention", "error", "warn", "ok", "all"];
/** Each choice's label key, in the reader's words like `LABEL`. */
const SHOWN_KEY: Record<Shown, string> = { attention: "attention", error: "broken", warn: "look", ok: "working", all: "all" };

function showsStatus(shown: Shown, status: Status): boolean {
  if (shown === "all") return true;
  if (shown === "attention") return status !== "ok";
  return status === shown;
}

/** The statuses of the last 7 days as a strip of bars, oldest first; read as a sentence. */
function History({ points, label }: { points: Check["history"]; label: string }): JSX.Element {
  if (points.length === 0) {
    return <span className="text-fg-muted">—</span>;
  }
  return (
    <span role="img" aria-label={label} className="flex h-4 items-end gap-px">
      {points.slice(-48).map((point) => (
        <span key={point.at} className={`w-1 rounded-sm ${BAR[point.status]}`} />
      ))}
    </span>
  );
}

/**
 * The health log (T-5067): every check of the project's pipelines, Context Spaces, Endpoints,
 * models and data sources, errors first, with since when it holds, what it found and how to fix
 * it. Without `project` it is the administrator's view across projects, with a project column.
 */
export function HealthLog({ project }: { project?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const say = useSay();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" });
  // A project holds hundreds of checks, almost all working: the list opens on what needs a look.
  const [shown, setShown] = useState<Shown | undefined>(undefined);
  const checks = useQuery({
    queryKey: project ? ["projects", project, "health-checks"] : ["organization", "health-checks"],
    queryFn: async () =>
      unwrap(
        project
          ? await api.GET("/api/v1/projects/{project}/health-checks", { params: { path: { project } } })
          : await api.GET("/api/v1/organization/health-checks"),
      ),
    refetchInterval: 60_000,
  });

  if (checks.isError) {
    return <PageFailed error={checks.error} onRetry={() => void checks.refetch()} />;
  }
  if (checks.isPending) {
    return (
      <Table caption={t("health.caption")} status={t("app.loading")}>
        <TableSkeleton columns={5} />
      </Table>
    );
  }
  const all = checks.data.items;
  if (all.length === 0) {
    return <EmptyState icon="refresh" title={t("health.empty")} description={t("health.emptyHint")} />;
  }
  const errors = all.filter((item) => item.status === "error").length;
  const warnings = all.filter((item) => item.status === "warn").length;
  const status: Shown = shown ?? (errors + warnings > 0 ? "attention" : "all");
  const counts: Record<Shown, number> = {
    attention: errors + warnings,
    error: errors,
    warn: warnings,
    ok: all.length - errors - warnings,
    all: all.length,
  };
  const items = all.filter((item) => showsStatus(status, item.status));
  return (
    <div className="flex flex-col gap-4">
      {errors + warnings > 0 ? (
        <Alert tone={errors > 0 ? "danger" : "warning"} role="status">
          {t("health.summary", { errors, warnings, total: all.length })}
        </Alert>
      ) : (
        <Alert tone="success" role="status">
          {t("health.allWell", { total: all.length })}
        </Alert>
      )}
      <div className="grid grid-cols-2 gap-3 sm:flex sm:flex-wrap sm:items-end">
        <Field id="health-status" label={t("health.statusFilter")} className="min-w-0 sm:w-56">
          <Select id="health-status" value={status} onChange={(event) => setShown(event.target.value as Shown)}>
            {SHOWN.map((value) => (
              <option key={value} value={value}>
                {t(`health.shown.${SHOWN_KEY[value]}`, { count: counts[value] })}
              </option>
            ))}
          </Select>
        </Field>
      </div>
      {items.length === 0 ? (
        <EmptyState bare icon="refresh" title={t("health.noneOfKind")} description={t("health.noneOfKindHint")} />
      ) : (
        <>
          {/* A phone reads one card per check: the grid stacks each row (T-5067 review). */}
          <ManifestGrid<Check>
            caption={t("health.caption")}
            storageKey="health-log"
            searchLabel={t("grid.searchIn", { list: t("health.caption") })}
            rows={items}
            rowKey={keyOf}
            query={checks}
            name={{
              id: "resource",
              label: t("health.resource"),
              text: (item) => `${item.name} (${item.project})`,
              cell: (item) => <Resource item={item} project={project} />,
            }}
            columns={[
              {
                id: "status",
                label: t("health.status"),
                filter: true,
                text: (item) => item.status,
                valueLabel: (value) => t(LABEL[value as Status]),
                cell: (item) => <StatusOf item={item} time={time} />,
              },
              { id: "kind", label: t("health.kind"), filter: true, text: (item) => item.kind, valueLabel: (value) => t(`health.kinds.${value}`), hidden: true },
              { id: "found", label: t("health.found"), text: (item) => (item.detail.text === "" ? t("health.nothingFound") : say(item.detail)), cell: (item) => <Found item={item} say={say} /> },
              {
                id: "history",
                label: t("health.history"),
                numeric: true,
                align: "right",
                // Sorted by the readings that needed a look.
                text: (item) => String(item.history.filter((point) => point.status !== "ok").length),
                cell: (item) => <HistoryOf item={item} />,
              },
              { id: "lastOk", label: t("health.lastOk"), text: (item) => item.lastOk ?? "", cell: (item) => <LastOk item={item} time={time} /> },
            ]}
            empty={<EmptyState bare icon="refresh" title={t("health.noneOfKind")} description={t("health.noneOfKindHint")} />}
          />
        </>
      )}
    </div>
  );
}

const keyOf = (item: Check): string => `${item.project}/${item.kind}/${item.name}/${item.check}`;

function StatusOf({ item, time }: { item: Check; time: Intl.DateTimeFormat }): JSX.Element {
  const { t } = useTranslation();
  return (
    <span className="flex flex-col items-start gap-1">
      <Badge tone={TONE[item.status]}>{t(LABEL[item.status])}</Badge>
      <span className="whitespace-nowrap text-caption text-fg-muted">
        {t("health.since", { at: time.format(new Date(item.since)) })}
      </span>
    </span>
  );
}

/** The resource's name as a link to its page, its kind and check, and its project in the admin view. */
function Resource({ item, project }: { item: Check; project?: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <span className="flex flex-col">
      <a href={safeHref(item.evidenceUrl)} className="focus-ring font-medium text-primary-soft-fg underline hover:no-underline">
        {item.name}
      </a>
      <span className="text-caption text-fg-muted">
        {t(`health.kinds.${item.kind}`)} · {(CHECKS as readonly string[]).includes(item.check) ? t(`health.checks.${item.check}`) : item.check}
      </span>
      {project ? null : (
        <span className="text-caption text-fg-muted">
          {t("health.project")}: {item.project}
        </span>
      )}
    </span>
  );
}

function Found({ item, say }: { item: Check; say: (text: Check["detail"]) => string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <span className="block max-w-md break-words">
      {item.detail.text === "" ? t("health.nothingFound") : say(item.detail)}
      {item.fix.text === "" ? null : (
        <span className="mt-1 block text-caption text-fg-muted">
          {t("health.fix")} {say(item.fix)}
        </span>
      )}
    </span>
  );
}

function HistoryOf({ item }: { item: Check }): JSX.Element {
  const { t } = useTranslation();
  return (
    <History
      points={item.history}
      label={t("health.historyLabel", {
        errors: item.history.filter((p) => p.status === "error").length,
        warnings: item.history.filter((p) => p.status === "warn").length,
        total: item.history.length,
      })}
    />
  );
}

function LastOk({ item, time }: { item: Check; time: Intl.DateTimeFormat }): JSX.Element {
  const { t } = useTranslation();
  return item.lastOk == null ? (
    <>{t("health.never")}</>
  ) : (
    <time dateTime={item.lastOk} className="whitespace-nowrap tabular-nums">
      {time.format(new Date(item.lastOk))}
    </time>
  );
}

/** What is broken in this project and what to do about it (T-5067). */
export function HealthPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("health.title")} description={t("health.lead")} />
      <HealthLog project={project} />
      {/* The project's calls per Endpoint and day, moved here from the Endpoints list (T-6095);
          each Endpoint's own page keeps its own. */}
      <ProjectUsageSection project={project} />
    </div>
  );
}
