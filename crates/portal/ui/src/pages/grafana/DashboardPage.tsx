/**
 * One dashboard of the project's daiya, drawn in the Portal (item 15): the panels where the
 * dashboard places them, a time range and a refresh interval, and the dashboard's variables. Each
 * panel reads its own data through the Portal, which runs the dashboard's saved queries only; the
 * page never sends a query. daiya's own `/d/{uid}` page stays the table view without script.
 */
import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, reasonOf, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Button, buttonClass, EmptyState, Field, Icon, PageFailed, PageHeader, PageLoading, Select, safeHref } from "../../components/ui";
import { bodyHeight, DashboardGrid } from "../../components/panels/DashboardGrid";
import type { Frame } from "../../components/panels/frames";
import type { PanelData, PanelDefinition } from "../../components/panels/PanelView";
import { PanelView } from "../../components/panels/PanelView";
import { RANGES, REFRESHES, rangeParts, refreshMs, resolveTime } from "../../components/panels/timeRange";

type View = components["schemas"]["DashboardView"];
type Variable = components["schemas"]["VariableView"];

/** What the page asks a panel for: the range as Grafana words, the picked value of each variable. */
export interface Asked {
  from: string;
  to: string;
  vars: Record<string, string>;
}

/** The address query of one panel read: `from`, `to`, `maxDataPoints` and `var-<name>` per variable. */
export function panelQuery(asked: Asked, maxDataPoints: number): string {
  const query = new URLSearchParams({ from: asked.from, to: asked.to, maxDataPoints: String(maxDataPoints) });
  for (const [name, value] of Object.entries(asked.vars)) query.append(`var-${name}`, value);
  return query.toString();
}

function rangeLabel(from: string, t: (key: string, options?: Record<string, unknown>) => string): string {
  const parts = rangeParts(from);
  return parts ? t(`dashboardView.ranges.${parts.n}${parts.unit}`, { defaultValue: from }) : from;
}

/** One panel and its own read, refetched on the refresh tick. */
function LivePanel({ project, uid, panel, asked, tick, locale, at }: { project: string; uid: string; panel: PanelDefinition; asked: Asked; tick: number; locale: string; at: number }): JSX.Element {
  const queryClient = useQueryClient();
  const key = ["projects", project, "grafana", "dashboards", uid, "panel", panel.id, asked, tick];
  const data = useQuery({
    queryKey: key,
    enabled: panel.type !== "text" && panel.type !== "row",
    placeholderData: (previous) => previous,
    queryFn: async (): Promise<PanelData> => {
      const answer = await unwrap(
        await api.GET("/api/v1/projects/{project}/grafana/dashboards/{uid}/panels/{panel}", {
          params: { path: { project, uid, panel: panel.id } },
          querySerializer: () => panelQuery(asked, panel.maxDataPoints ?? 1000),
        }),
      );
      // The wire names frames only as objects; their shape is daiya's (frames.ts).
      return { frames: answer.frames as unknown as Frame[], accessDenied: answer.accessDenied ?? undefined, error: answer.error ?? undefined };
    },
  });
  const from = resolveTime(asked.from, at) ?? at - 6 * 3_600_000;
  const to = resolveTime(asked.to, at) ?? at;
  return (
    <PanelView
      panel={panel}
      data={data.data}
      loading={data.isPending}
      failure={data.isError ? reasonOf(data.error) : undefined}
      onRetry={() => void queryClient.invalidateQueries({ queryKey: key.slice(0, 7) })}
      from={from}
      to={to}
      locale={locale}
      height={bodyHeight(panel)}
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

function VariablePicker({ project, uid, variable, value, onChange }: { project: string; uid: string; variable: Variable; value: string; onChange: (value: string) => void }): JSX.Element {
  const { t } = useTranslation();
  const read = useVariableOptions(project, uid, variable);
  const options = variable.kind === "query" ? (read.data ?? []) : variable.options;
  const id = `dashboard-var-${variable.name}`;
  return (
    <Field id={id} label={variable.label || variable.name} help={read.isError ? reasonOf(read.error) : undefined}>
      <Select id={id} value={value} onChange={(event) => onChange(event.target.value)} disabled={read.isPending && variable.kind === "query"} title={read.isPending && variable.kind === "query" ? t("dashboardView.readingValues") : undefined}>
        {variable.includeAll ? <option value="$__all">{t("dashboardView.all")}</option> : null}
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.text}
          </option>
        ))}
        {value && value !== "$__all" && !options.some((option) => option.value === value) ? <option value={value}>{value}</option> : null}
      </Select>
    </Field>
  );
}

export function DashboardPage({ project, uid }: { project: string; uid: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const view = useQuery({
    queryKey: ["projects", project, "grafana", "dashboards", uid],
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/grafana/dashboards/{uid}", { params: { path: { project, uid } } })),
  });
  const [range, setRange] = useState<string | null>(null);
  const [refresh, setRefresh] = useState<string | null>(null);
  const [vars, setVars] = useState<Record<string, string>>({});
  const [tick, setTick] = useState(0);
  const [at, setAt] = useState(() => Date.now());
  const data: View | undefined = view.data;
  const every = refreshMs(refresh ?? data?.refresh ?? "");
  useEffect(() => {
    if (!every) return undefined;
    const timer = window.setInterval(() => {
      setAt(Date.now());
      setTick((n) => n + 1);
    }, every);
    return () => window.clearInterval(timer);
  }, [every]);
  const asked: Asked = useMemo(() => {
    const picked: Record<string, string> = {};
    for (const variable of data?.variables ?? []) {
      const value = vars[variable.name] ?? variable.current ?? "";
      if (value !== "") picked[variable.name] = value;
    }
    return { from: range ?? data?.from ?? "now-6h", to: range ? "now" : (data?.to ?? "now"), vars: picked };
  }, [data, range, vars]);
  const fallback = safeHref(`/dashboards/d/${encodeURIComponent(uid)}`);
  const header = (
    <PageHeader
      title={data?.title || t("dashboardView.title")}
      description={data?.description || t("dashboardView.lead")}
      actions={
        fallback ? (
          <a className={buttonClass("ghost", "sm")} href={fallback} target="_blank" rel="noopener noreferrer">
            <Icon name="external" className="size-4" />
            {t("dashboardView.asTables")}
          </a>
        ) : null
      }
    />
  );
  if (view.isPending) return <PageLoading label={t("dashboardView.title")} />;
  if (view.isError || !data) {
    return (
      <div className="flex flex-col gap-section">
        {header}
        <PageFailed error={view.error} onRetry={() => void view.refetch()} />
      </div>
    );
  }
  const ranges: string[] = RANGES.includes(asked.from as (typeof RANGES)[number]) ? [...RANGES] : [asked.from, ...RANGES];
  const panels = data.panels as unknown as PanelDefinition[];
  return (
    <div className="flex flex-col gap-section">
      {header}
      <div className="flex flex-wrap items-end gap-3" role="group" aria-label={t("dashboardView.controls")}>
        <Field id="dashboard-range" label={t("dashboardView.rangeLabel")}>
          <Select id="dashboard-range" value={asked.from} onChange={(event) => {
              setAt(Date.now());
              setRange(event.target.value);
            }}>
            {ranges.map((from) => (
              <option key={from} value={from}>
                {rangeLabel(from, t)}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="dashboard-refresh" label={t("dashboardView.refreshLabel")}>
          <Select id="dashboard-refresh" value={refresh ?? data.refresh ?? ""} onChange={(event) => setRefresh(event.target.value)}>
            {REFRESHES.map((every) => (
              <option key={every} value={every}>
                {every ? t("dashboardView.every", { every }) : t("dashboardView.refreshOff")}
              </option>
            ))}
          </Select>
        </Field>
        {data.variables.map((variable) => (
          <VariablePicker
            key={variable.name}
            project={project}
            uid={uid}
            variable={variable}
            value={asked.vars[variable.name] ?? ""}
            onChange={(value) => setVars({ ...vars, [variable.name]: value })}
          />
        ))}
        <Button
          variant="secondary"
          onClick={() => {
            setAt(Date.now());
            setTick((n) => n + 1);
          }}
        >
          <Icon name="refresh" className="size-4" />
          {t("dashboardView.refreshNow")}
        </Button>
      </div>
      {panels.length === 0 ? (
        <EmptyState icon="dashboards" title={t("dashboardView.noPanelsTitle")} description={t("dashboardView.noPanels")} />
      ) : (
        <DashboardGrid
          panels={panels}
          render={(panel) => <LivePanel project={project} uid={uid} panel={panel} asked={asked} tick={tick} locale={locale} at={at} />}
        />
      )}
    </div>
  );
}
