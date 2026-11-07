/**
 * A chart from the explorer (UI-93, T-3257): pick an attribute of the type, take the chart its type
 * suggests (its history, bars by value, a histogram), see it, and save it as a widget to a
 * dashboard of the project or to a new one. The save is a proposed change like every other write;
 * the widget reads through the same endpoint with the view's filter.
 */
import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { TemporalChart } from "../../components/dashboards/TemporalChart";
import { TypeChart } from "../../components/dashboards/TypeChart";
import { suggestChart } from "../../components/dashboards/typeCharts";
import type { ChartKind } from "../../components/dashboards/typeCharts";
import { fetchEntities } from "../../components/entities/filters";
import type { FilterSlot } from "../../components/entities/filters";
import { Alert, Button, Dialog, Field, Input, Select } from "../../components/ui";
import { RadioGroup } from "../../components/ui/RadioGroup";

const NEW = "";
const KINDS: ChartKind[] = ["temporal-chart", "bar-chart", "histogram"];

/** The widget a chart is saved as (Architecture/10 §1). */
export function chartWidget(
  kind: ChartKind,
  view: { endpoint: string; type: string; q?: string },
  property: string,
  entityId?: string,
): Record<string, unknown> {
  if (kind === "temporal-chart") return { widgetType: kind, endpointRef: view.endpoint, entityId, property };
  return { widgetType: kind, endpointRef: view.endpoint, entityType: view.type, property, ...(view.q?.trim() ? { q: view.q.trim() } : {}) };
}

/** The dashboard with the widget added: to the last page that holds widgets, or on a page of its own. */
export function withWidget(dashboard: Manifest, widget: Record<string, unknown>, pageTitle: string): Manifest {
  const spec = dashboard.spec as { pages?: { widgets?: unknown[] }[] };
  const pages = [...(spec.pages ?? [])];
  const last = pages.map((page, index) => ((page.widgets ?? []).length > 0 ? index : -1)).filter((index) => index >= 0).pop();
  if (last === undefined) pages.push({ title: pageTitle, layout: "grid-2x2", widgets: [widget] } as { widgets: unknown[] });
  else pages[last] = { ...pages[last], widgets: [...(pages[last].widgets ?? []), widget] };
  return { ...dashboard, spec: { ...dashboard.spec, pages } } as Manifest;
}

/** A new dashboard holding the one widget. */
export function newDashboard(project: string, name: string, title: string, widget: Record<string, unknown>, pageTitle: string): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Dashboard",
    metadata: { name, namespace: project },
    spec: { title, visibility: "project", pages: [{ title: pageTitle, layout: "grid-2x2", widgets: [widget] }] },
  } as Manifest;
}

const DNS1123 = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

export function ChartFromView({
  project,
  endpoint,
  slug,
  type,
  q,
  slots,
}: {
  project: string;
  /** The endpoint's manifest name, which the widget names. */
  endpoint: string;
  slug: string;
  type: string;
  /** The view's filter, both of them joined. */
  q?: string;
  slots: FilterSlot[];
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [property, setProperty] = useState("");
  const [chosen, setChosen] = useState<ChartKind | null>(null);
  const [entityId, setEntityId] = useState("");
  const [target, setTarget] = useState(NEW);
  const [name, setName] = useState(`${type.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 50) || "charts"}-charts`);
  const [change, setChange] = useState<Change | null>(null);
  const plain = slots.filter((slot) => slot.kind !== "GeoProperty" && slot.kind !== "Relationship");

  // A few entities with the attribute, normalized: whether it is observed over time, and the
  // entities a history can be drawn for.
  const samples = useQuery({
    queryKey: ["chart-samples", slug, type, property, q],
    enabled: open && property !== "",
    retry: false,
    queryFn: async () => (await fetchEntities(slug, { type, q, attrs: [property] }, { limit: 20, keyValues: false })).rows as Record<string, unknown>[],
  });
  const dashboards = useQuery({
    queryKey: queryKeys.list(project, "dashboards"),
    enabled: open,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "dashboards" } } })),
    select: (list) => asManifests(list.items ?? []),
  });

  const suggested = property ? suggestChart(plain.find((slot) => slot.name === property), samples.data ?? [], property) : null;
  const kind = chosen ?? suggested;
  const ids = (samples.data ?? []).map((entity) => String(entity.id));
  const entity = entityId || ids[0] || "";
  const widget = kind && property ? chartWidget(kind, { endpoint, type, q }, property, entity) : null;
  const pageTitle = t("explore.chart.page");

  const save = useMutation({
    mutationFn: async () => {
      if (!widget) throw new Error(t("explore.chart.pickFirst"));
      if (target === NEW) {
        if (!DNS1123.test(name)) throw new Error(t("explore.chart.badName"));
        const manifest = newDashboard(project, name, t("explore.chart.newTitle", { type }), widget, pageTitle);
        return (await proposeChecked(project, "dashboards", manifest as ResourceProposal, true)) as Change;
      }
      const dashboard = (dashboards.data ?? []).find((each) => each.metadata.name === target);
      if (!dashboard) throw new Error(t("explore.chart.gone"));
      return (await proposeChecked(project, "dashboards", withWidget(dashboard, widget, pageTitle) as ResourceProposal, false)) as Change;
    },
    onSuccess: (proposed) => {
      setChange(proposed);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "dashboards") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const reason = save.error instanceof ApiError ? (save.error.problem?.detail ?? save.error.message) : save.error instanceof Error ? save.error.message : null;

  return (
    <>
      <Button size="sm" variant="secondary" onClick={() => setOpen(true)}>
        {t("explore.chart.open")}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            setChange(null);
            save.reset();
          }
        }}
        title={t("explore.chart.title", { type })}
        description={t("explore.chart.lead")}
        closeLabel={t("app.close")}
        footer={
          change ? undefined : (
            <Button
              disabled={!widget || save.isPending || (kind === "temporal-chart" && !entity)}
              disabledReason={
                save.isPending
                  ? t("app.busyReason")
                  : !widget
                    ? t("explore.chart.pickAttribute")
                    : t("explore.chart.pickEntity")
              }
              onClick={() => save.mutate()}
            >
              {t("explore.chart.save")}
            </Button>
          )
        }
      >
        <div className="space-y-4">
          <Field id={`${id}-attr`} label={t("explore.chart.attribute")}>
            <Select
              id={`${id}-attr`}
              value={property}
              onChange={(event) => {
                setProperty(event.target.value);
                setChosen(null);
                setEntityId("");
              }}
            >
              <option value="">{t("explore.chart.pickAttribute")}</option>
              {plain.map((slot) => (
                <option key={slot.name} value={slot.name}>
                  {slot.name}
                </option>
              ))}
            </Select>
          </Field>
          {property && kind ? (
            <RadioGroup<ChartKind>
              name={`${id}-kind`}
              legend={t("explore.chart.kind")}
              description={suggested ? t("explore.chart.suggested", { kind: t(`explore.chart.kinds.${suggested}.label`) }) : undefined}
              value={kind}
              onChange={setChosen}
              layout="row"
              options={KINDS.map((value) => ({
                value,
                label: t(`explore.chart.kinds.${value}.label`),
                description: t(`explore.chart.kinds.${value}.says`),
              }))}
            />
          ) : null}
          {kind === "temporal-chart" && ids.length > 0 ? (
            <Field id={`${id}-entity`} label={t("explore.chart.entity")}>
              <Select id={`${id}-entity`} value={entity} onChange={(event) => setEntityId(event.target.value)}>
                {ids.map((each) => (
                  <option key={each} value={each}>
                    {each}
                  </option>
                ))}
              </Select>
            </Field>
          ) : null}
          {widget && kind === "temporal-chart" && entity ? (
            <TemporalChart slug={slug} entityId={entity} property={property} title={t("dashboards.widget.temporalChart", { property })} />
          ) : widget && (kind === "bar-chart" || kind === "histogram") ? (
            <TypeChart
              slug={slug}
              entityType={type}
              property={property}
              q={q}
              kind={kind}
              title={t(`dashboards.widget.${kind === "bar-chart" ? "barTitle" : "histogramTitle"}`, { property, type })}
            />
          ) : null}
          {widget && !change ? (
            <div className="grid gap-3 sm:grid-cols-2">
              <Field id={`${id}-target`} label={t("explore.chart.dashboard")}>
                <Select id={`${id}-target`} value={target} onChange={(event) => setTarget(event.target.value)}>
                  <option value={NEW}>{t("explore.chart.newDashboard")}</option>
                  {(dashboards.data ?? []).map((each) => (
                    <option key={each.metadata.name} value={each.metadata.name}>
                      {each.metadata.name}
                    </option>
                  ))}
                </Select>
              </Field>
              {target === NEW ? (
                <Field id={`${id}-name`} label={t("explore.chart.name")} description={t("explore.chart.nameHelp")}>
                  <Input id={`${id}-name`} value={name} onChange={(event) => setName(event.target.value.trim())} />
                </Field>
              ) : null}
            </div>
          ) : null}
          {reason ? (
            <Alert tone="danger" role="alert">
              {reason}
            </Alert>
          ) : null}
          {change ? <ChangeNotice change={change} project={project} /> : null}
        </div>
      </Dialog>
    </>
  );
}
