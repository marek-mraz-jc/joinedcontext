/**
 * One dashboard of the project's daiya, drawn in the Portal (item 15): the panels where the
 * dashboard places them, a time range and a refresh interval, and the dashboard's variables. Each
 * panel reads its own data through the Portal, which runs the dashboard's saved queries only; the
 * page never sends a query. daiya's own `/d/{uid}` page stays the table view without script.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { api, ApiError, reasonOf, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Alert, Button, buttonClass, Checkbox, EmptyState, Field, Icon, PageFailed, PageHeader, PageLoading, Select, safeHref } from "../../components/ui";
import { ToolbarPopover } from "../../components/entities/GridToolbar";
import { bodyHeight, DashboardGrid } from "../../components/panels/DashboardGrid";
import type { Frame } from "../../components/panels/frames";
import type { PanelData, PanelDefinition } from "../../components/panels/PanelView";
import { PanelView } from "../../components/panels/PanelView";
import { overrideWords, RefreshPicker, rangeWords, TimeRangeControls } from "../../components/panels/TimePicker";
import { panelRange, QUICK_RANGES, refreshMs, resolveTime } from "../../components/panels/timeRange";

type View = components["schemas"]["DashboardView"];
type Variable = components["schemas"]["VariableView"];

/** What the page asks a panel for: the range as Grafana words, the picked values of each variable. */
export interface Asked {
  from: string;
  to: string;
  vars: Record<string, string[]>;
}

/**
 * The address query of one panel read, as Grafana asks its datasources: the panel's own range as
 * epoch milliseconds (its `timeFrom` and `timeShift` applied), `maxDataPoints`, and one
 * `var-<name>` per picked value.
 */
export function panelQuery(range: { from: number; to: number }, vars: Asked["vars"], maxDataPoints: number): string {
  const query = new URLSearchParams({ from: String(range.from), to: String(range.to), maxDataPoints: String(maxDataPoints) });
  for (const [name, values] of Object.entries(vars)) for (const value of values) query.append(`var-${name}`, value);
  return query.toString();
}

interface LivePanelProps {
  project: string;
  uid: string;
  panel: PanelDefinition;
  asked: Asked;
  tick: number;
  locale: string;
  at: number;
  /** The range and variables in words, for an empty panel. */
  words: { range: string; vars?: string };
  onWiden?: () => void;
  onZoom?: (from: number, to: number) => void;
  /** Tells the page whether this panel failed, so several failures make one notice. */
  onFailed: (id: number, failed: boolean) => void;
  quiet: boolean;
}

/** One panel and its own read, refetched on the refresh tick. */
function LivePanel({ project, uid, panel, asked, tick, locale, at, words, onWiden, onZoom, onFailed, quiet }: LivePanelProps): JSX.Element {
  const queryClient = useQueryClient();
  const range = panelRange(panel, asked, at);
  const key = ["projects", project, "grafana", "dashboards", uid, "panel", panel.id, asked, tick];
  const data = useQuery({
    queryKey: key,
    enabled: panel.type !== "text" && panel.type !== "row",
    placeholderData: (previous) => previous,
    queryFn: async (): Promise<PanelData> => {
      const answer = await unwrap(
        await api.GET("/api/v1/projects/{project}/grafana/dashboards/{uid}/panels/{panel}", {
          params: { path: { project, uid, panel: panel.id } },
          querySerializer: () => panelQuery(range, asked.vars, panel.maxDataPoints ?? 1000),
        }),
      );
      // The wire names frames only as objects; their shape is daiya's (frames.ts).
      return { frames: answer.frames as unknown as Frame[], accessDenied: answer.accessDenied ?? undefined, error: answer.error ?? undefined };
    },
  });
  const failed = data.isError || Boolean(data.data?.error);
  useEffect(() => onFailed(panel.id, failed), [onFailed, panel.id, failed]);
  const { t } = useTranslation();
  return (
    <PanelView
      panel={panel}
      data={data.data}
      loading={data.isPending}
      failure={data.isError ? reasonOf(data.error) : undefined}
      onRetry={() => void queryClient.invalidateQueries({ queryKey: key.slice(0, 7) })}
      from={range.from}
      to={range.to}
      locale={locale}
      height={bodyHeight(panel)}
      asked={words}
      onWiden={onWiden}
      onZoom={onZoom}
      quiet={quiet && failed}
      timeInfo={panel.hideTimeOverride ? undefined : overrideWords(range, t)}
    />
  );
}

/** A query variable's values, read from its datasource through the Portal. */
function useVariableOptions(project: string, uid: string, variable: Variable) {
  return useQuery({
    queryKey: ["projects", project, "grafana", "dashboards", uid, "variable", variable.name],
    enabled: variable.kind === "query",
    queryFn: async () =>
      (
        await unwrap(
          await api.GET("/api/v1/projects/{project}/grafana/dashboards/{uid}/variables/{name}", {
            params: { path: { project, uid, name: variable.name } },
          }),
        )
      ).options,
  });
}

/**
 * A multi-value pick, Grafana's way: `All` stands alone, a value picked under `All` replaces it,
 * and nothing left picked falls back to `All` where the variable offers it.
 */
export function toggled(picked: string[], value: string, includeAll: boolean): string[] {
  if (value === "$__all") return ["$__all"];
  const rest = picked.filter((each) => each !== "$__all");
  const next = rest.includes(value) ? rest.filter((each) => each !== value) : [...rest, value];
  return next.length === 0 && includeAll ? ["$__all"] : next;
}

function VariablePicker({ project, uid, variable, value, onChange }: { project: string; uid: string; variable: Variable; value: string[]; onChange: (value: string[]) => void }): JSX.Element {
  const { t } = useTranslation();
  const read = useVariableOptions(project, uid, variable);
  const options = variable.kind === "query" ? (read.data ?? []) : variable.options;
  const id = `dashboard-var-${variable.name}`;
  const label = variable.label || variable.name;
  if (variable.multi) {
    const choices = [...(variable.includeAll ? [{ text: t("dashboardView.all"), value: "$__all" }] : []), ...options];
    const words = value.map((each) => (each === "$__all" ? t("dashboardView.all") : (options.find((option) => option.value === each)?.text ?? each))).join(" + ");
    return (
      <div className="flex flex-col gap-1">
        <span className="text-body font-medium text-fg">{label}</span>
        <ToolbarPopover label={words || t("dashboardView.none")} outlined anchored closeLabel={t("dashboardView.timePicker.close")}>
          <fieldset aria-label={label} className="flex max-h-80 flex-col gap-2 overflow-y-auto">
            {read.isPending && variable.kind === "query" ? <p className="text-caption text-fg-muted">{t("dashboardView.readingValues")}</p> : null}
            {read.isError ? <p className="text-caption text-danger">{reasonOf(read.error)}</p> : null}
            {choices.map((option) => (
              <Checkbox key={option.value} label={option.text} checked={value.includes(option.value)} onChange={() => onChange(toggled(value, option.value, variable.includeAll))} />
            ))}
          </fieldset>
        </ToolbarPopover>
      </div>
    );
  }
  const one = value[0] ?? "";
  return (
    <Field id={id} label={label} help={read.isError ? reasonOf(read.error) : undefined}>
      <Select id={id} value={one} onChange={(event) => onChange([event.target.value])} disabled={read.isPending && variable.kind === "query"} title={read.isPending && variable.kind === "query" ? t("dashboardView.readingValues") : undefined}>
        {variable.includeAll ? <option value="$__all">{t("dashboardView.all")}</option> : null}
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.text}
          </option>
        ))}
        {one && one !== "$__all" && !options.some((option) => option.value === one) ? <option value={one}>{one}</option> : null}
      </Select>
    </Field>
  );
}

/** A uid the builder minted (`jcd-` and 16 hex digits), the only kind it edits. */
export function isBuilderUid(uid: string): boolean {
  return /^jcd-[0-9a-f]{16}$/.test(uid);
}

/** A search member as text: TanStack reads `from=1762581600000` as a number. */
function word(value: unknown): string | undefined {
  return typeof value === "string" || typeof value === "number" ? String(value) : undefined;
}

/** The next quick range out from `from`..`to`, for an empty panel's "Show a longer range". */
function longerThan(from: string, to: string, now: number): string | undefined {
  const start = resolveTime(from, now);
  const end = resolveTime(to, now, true);
  if (start === null || end === null) return undefined;
  return QUICK_RANGES.find((range) => range.to === "now" && (resolveTime(range.from, now) ?? now) < start && now >= end)?.from;
}

export function DashboardPage({ project, uid }: { project: string; uid: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const view = useQuery({
    queryKey: ["projects", project, "grafana", "dashboards", uid],
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/grafana/dashboards/{uid}", { params: { path: { project, uid } } })),
  });
  // The range and refresh live in the address, as Grafana keeps them (`?from=&to=&refresh=`).
  const search = useSearch({ strict: false }) as Record<string, unknown>;
  const navigate = useNavigate();
  const link = useCallback(
    (next: Record<string, string | undefined>) =>
      void navigate({ to: ".", search: (previous: Record<string, unknown>) => ({ ...previous, ...next }), replace: true }),
    [navigate],
  );
  const [vars, setVars] = useState<Record<string, string[]>>({});
  const [tick, setTick] = useState(0);
  const [at, setAt] = useState(() => Date.now());
  const queryClient = useQueryClient();
  const [failing, setFailing] = useState<Record<number, boolean>>({});
  const onFailed = useCallback((id: number, failed: boolean) => setFailing((now) => (Boolean(now[id]) === failed ? now : { ...now, [id]: failed })), []);
  const data: View | undefined = view.data;
  const from = word(search.from) ?? data?.from ?? "now-6h";
  const to = word(search.to) ?? (search.from === undefined ? data?.to : undefined) ?? "now";
  const refresh = word(search.refresh) ?? data?.refresh ?? "";
  const span = (resolveTime(to, at, true) ?? at) - (resolveTime(from, at) ?? at);
  const every = data?.timePickerHidden ? 0 : refreshMs(refresh, span, typeof window === "undefined" ? 2000 : window.innerWidth);
  useEffect(() => {
    if (!every) return undefined;
    const timer = window.setInterval(() => {
      setAt(Date.now());
      setTick((n) => n + 1);
    }, every);
    return () => window.clearInterval(timer);
  }, [every]);
  const asked: Asked = useMemo(() => {
    const picked: Record<string, string[]> = {};
    for (const variable of data?.variables ?? []) {
      const values = vars[variable.name] ?? variable.current;
      if (values.length) picked[variable.name] = values;
    }
    return { from, to, vars: picked };
  }, [data, from, to, vars]);
  // Only a dashboard the builder wrote opens in the builder, and only for a person who may propose one.
  const builder = useQuery({
    queryKey: ["projects", project, "grafana", "builder"],
    enabled: isBuilderUid(uid),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/grafana/builder", { params: { path: { project } } })),
  });
  const mayEdit = isBuilderUid(uid) && builder.data?.mayEdit === true;
  // The tables page lives in the dashboards service, in the organization that holds the dashboard.
  const safe = data
    ? safeHref(`/dashboards/d/${encodeURIComponent(uid)}?${new URLSearchParams({ orgId: String(data.orgId), from, to })}`)
    : undefined;
  const header = (
    <PageHeader
      title={data?.title || t("dashboardView.title")}
      description={data?.description || t("dashboardView.lead")}
      actions={
        <>
          {mayEdit ? (
            <Link to="/projects/$project/grafana/dashboards/$uid/edit" params={{ project, uid }} className={buttonClass("secondary", "sm")}>
              <Icon name="pencil" className="size-4" />
              {t("builder.edit")}
            </Link>
          ) : null}
          {safe ? (
            <a className={buttonClass("ghost", "sm")} href={safe} target="_blank" rel="noopener noreferrer">
              <Icon name="external" className="size-4" />
              {t("dashboardView.asTables")}
            </a>
          ) : null}
        </>
      }
    />
  );
  if (view.isPending) return <PageLoading label={t("dashboardView.title")} />;
  // An answer without its panels or variables (an older Portal, a stub) is a failed read, not a crash.
  if (view.isError || !data || !Array.isArray(data.panels) || !Array.isArray(data.variables)) {
    const bare = <PageHeader title={t("dashboardView.title")} description={t("dashboardView.lead")} />;
    if (view.error instanceof ApiError && view.error.status === 404) {
      return (
        <div className="flex flex-col gap-section">
          {bare}
          <EmptyState
            icon="dashboards"
            title={t("dashboardView.missingTitle")}
            description={t("dashboardView.missing")}
            action={
              <Link to="/projects/$project/grafana" params={{ project }} className={buttonClass("secondary", "sm")}>
                {t("dashboardView.backToList")}
              </Link>
            }
          />
        </div>
      );
    }
    return (
      <div className="flex flex-col gap-section">
        {bare}
        <PageFailed error={view.error} onRetry={() => void view.refetch()} />
      </div>
    );
  }
  const failed = Object.values(failing).filter(Boolean).length;
  const longer = data.timePickerHidden ? undefined : longerThan(from, to, at);
  const picked = data.variables
    .map((variable) => {
      const values = asked.vars[variable.name];
      return values ? `${variable.label || variable.name} ${values.map((value) => (value === "$__all" ? t("dashboardView.all") : value)).join(" + ")}` : null;
    })
    .filter(Boolean)
    .join(", ");
  const words = { range: rangeWords(from, to, at, t), ...(picked ? { vars: picked } : {}) };
  const panels = data.panels as unknown as PanelDefinition[];
  const refreshNow = () => {
    setAt(Date.now());
    setTick((n) => n + 1);
  };
  return (
    <div className="flex flex-col gap-section">
      {header}
      <div className="flex flex-wrap items-end gap-3" role="group" aria-label={t("dashboardView.controls")}>
        {data.variables.map((variable) => (
          <VariablePicker
            key={variable.name}
            project={project}
            uid={uid}
            variable={variable}
            value={asked.vars[variable.name] ?? []}
            onChange={(value) => setVars({ ...vars, [variable.name]: value })}
          />
        ))}
        {data.timePickerHidden ? null : (
          <>
            <TimeRangeControls
              from={from}
              to={to}
              now={at}
              onChange={(nextFrom, nextTo) => {
                setAt(Date.now());
                link({ from: nextFrom, to: nextTo });
              }}
            />
            <RefreshPicker value={refresh} onChange={(next) => link({ refresh: next || undefined })} onRefresh={refreshNow} />
          </>
        )}
      </div>
      {failed > 1 ? (
        <Alert
          tone="danger"
          title={t("dashboardView.failing", { n: failed })}
          actions={
            <Button size="sm" variant="secondary" onClick={() => void queryClient.invalidateQueries({ queryKey: ["projects", project, "grafana", "dashboards", uid, "panel"] })}>
              <Icon name="refresh" className="size-4" />
              {t("dashboardView.retryAll")}
            </Button>
          }
        >
          {t("dashboardView.failingHint")}
        </Alert>
      ) : null}
      {panels.length === 0 ? (
        <EmptyState icon="dashboards" title={t("dashboardView.noPanelsTitle")} description={t("dashboardView.noPanels")} />
      ) : (
        <DashboardGrid
          panels={panels}
          render={(panel) => (
            <LivePanel
              project={project}
              uid={uid}
              panel={panel}
              asked={asked}
              tick={tick}
              locale={locale}
              at={at}
              words={words}
              onWiden={
                longer
                  ? () => {
                      setAt(Date.now());
                      link({ from: longer, to: "now" });
                    }
                  : undefined
              }
              onZoom={(from, to) => link({ from: String(from), to: String(to) })}
              onFailed={onFailed}
              quiet={failed > 1}
            />
          )}
        />
      )}
    </div>
  );
}
