/**
 * A dashboard written as JSON (T-6791): the manifest on the left, completed and checked against the
 * Dashboard kind's schema, its widgets drawn on the right as the dashboards page draws them. A
 * picker inserts a valid widget over an Endpoint the person may read; Check asks the server for a
 * dry run and lists each refusal with its JSON path; Propose sends the manifest as a Change.
 */
import { lazy, Suspense, useId, useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, ApiError, problemLines, queryKeys, unwrap } from "../../api/client";
import { asManifests, refName } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { useProposal } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DashboardWidgets } from "../../components/dashboards/DashboardWidgets";
import { Alert, Button, Card, EmptyState, Field, PageHeader, Select, Skeleton } from "../../components/ui";
import { entityTypesOf, spaceOf } from "../spaces/spaceFacts";
import {
  insertWidget,
  parseManifest,
  PICKED_WIDGETS,
  schemaWithEndpoints,
  starter,
  toText,
  widgetSnippet,
} from "./dashboardJson";
import type { PickedWidget } from "./dashboardJson";

const DashboardJsonMonaco = lazy(() => import("./DashboardJsonMonaco"));

/** A finding of the server's dry run. */
interface Finding {
  level?: string;
  message?: string;
  path?: string;
}

function useList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural } } })),
  });
}

export function DashboardJsonEditor({ project, name }: { project: string; name?: string }): JSX.Element {
  const { t } = useTranslation();
  const ids = { type: useId(), endpoint: useId(), entity: useId(), editor: useId(), why: useId() };
  const dashboards = useList(project, "dashboards");
  const endpoints = useList(project, "endpoints");
  const spaces = useList(project, "spaces");
  const models = useList(project, "datamodels");
  const schema = useQuery({
    queryKey: ["kinds", "Dashboard", "schema"],
    queryFn: async () =>
      (await unwrap(await api.GET("/api/v1/kinds/{kind}/schema", { params: { path: { kind: "Dashboard" } } }))) as Record<
        string,
        unknown
      >,
    staleTime: Infinity,
  });

  const endpointManifests = useMemo(() => asManifests(endpoints.data?.items ?? []), [endpoints.data]);
  const existing = useMemo(
    () =>
      name
        ? asManifests(dashboards.data?.items ?? []).find((d) => d.kind === "Dashboard" && d.metadata.name === name)
        : undefined,
    [dashboards.data, name],
  );

  // The text is the source of truth; until the person types, it follows what the server holds.
  const [typed, setTyped] = useState<string | null>(null);
  const loaded = existing
    ? toText({ apiVersion: existing.apiVersion, kind: existing.kind, metadata: { name: existing.metadata.name, namespace: project }, spec: existing.spec })
    : toText(starter(project));
  const text = typed ?? loaded;
  const parsed = useMemo(() => parseManifest(text), [text]);

  const endpointNames = useMemo(() => endpointManifests.map((e) => e.metadata.name), [endpointManifests]);
  const editorSchema = useMemo(
    () => (schema.data ? schemaWithEndpoints(schema.data, endpointNames) : null),
    [schema.data, endpointNames],
  );

  // The picker: a widget type, an Endpoint the person may read, a type of the Endpoint's model.
  const [pickedType, setPickedType] = useState<PickedWidget>("vega-lite");
  const [inserted, setInserted] = useState(false);
  const [pickedEndpoint, setPickedEndpoint] = useState("");
  const [pickedEntity, setPickedEntity] = useState("");
  const endpoint = pickedEndpoint || endpointNames[0] || "";
  const entityTypes = useMemo(() => {
    const chosen = endpointManifests.find((e) => e.metadata.name === endpoint);
    const space = chosen
      ? asManifests(spaces.data?.items ?? []).find((s) => s.metadata.name === spaceOf(chosen))
      : undefined;
    const model = asManifests(models.data?.items ?? []).find(
      (m) => m.metadata.name === refName(space?.spec.dataModelRef),
    );
    return model ? entityTypesOf(model) : [];
  }, [endpointManifests, endpoint, spaces.data, models.data]);
  const entityType = entityTypes.includes(pickedEntity) ? pickedEntity : (entityTypes[0] ?? "");
  const canInsert = Boolean(parsed.manifest && endpoint && entityType);

  // The preview: every page's widgets, each reading through its Endpoint's slug. A public
  // dashboard draws only from public Endpoints (UI-19), as the dashboards page does.
  const pages = useMemo(() => {
    const byName = new Map(endpointManifests.map((e) => [e.metadata.name, e] as const));
    const spec = parsed.manifest?.spec;
    return (spec?.pages ?? []).map((page, index) => {
      const refused: string[] = [];
      const widgets = (Array.isArray(page.widgets) ? page.widgets : []).flatMap((widget) => {
        const read = widget?.endpointRef ? byName.get(widget.endpointRef) : undefined;
        const readSpec = (read?.spec ?? {}) as { slug?: string; audience?: string };
        if (spec?.visibility === "public" && readSpec.audience !== "public") {
          refused.push(widget?.endpointRef ?? widget?.widgetType ?? "");
          return [];
        }
        return widget && typeof widget.widgetType === "string" ? [{ widget, slug: readSpec.slug }] : [];
      });
      return { key: `${index}-${page.title ?? ""}`, title: page.title, widgets, refused };
    });
  }, [parsed.manifest, endpointManifests]);

  const [change, setChange] = useState<Change | null>(null);
  const proposal = useProposal(project, "dashboards", setChange);
  const check = useMutation({
    mutationFn: async (manifest: ResourceProposal) => {
      const create = !existing;
      const query = { dryRun: "All" };
      const answer = create
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "dashboards" }, query },
            body: manifest,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural: "dashboards", name: manifest.metadata.name }, query },
            body: manifest,
          });
      return (await unwrap(answer)) as { valid?: boolean; verdict?: { ok?: boolean; findings?: Finding[] } };
    },
  });
  const refusals: Finding[] = check.error
    ? check.error instanceof ApiError
      ? problemLines(check.error.problem).map((line) => ({ level: "error", message: line }))
      : [{ level: "error", message: check.error.message }]
    : (check.data?.verdict?.findings ?? []).filter((f) => f.level === "error" || !f.level);
  const checkedGreen = check.isSuccess && check.data?.valid !== false && refusals.length === 0;

  const asProposal = (): ResourceProposal | null =>
    parsed.manifest ? (parsed.manifest as unknown as ResourceProposal) : null;
  const loading = dashboards.isPending || endpoints.isPending;

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={existing ? t("dashboards.json.editTitle", { name }) : t("dashboards.json.newTitle")}
        description={t("dashboards.json.lead")}
        breadcrumb={
          <Link to="/projects/$project/$plural" params={{ project, plural: "dashboards" }}>
            {t("dashboards.title")}
          </Link>
        }
      />
      {change ? <ChangeNotice change={change} project={project} /> : null}
      {loading ? (
        <Skeleton className="h-96" />
      ) : endpointNames.length === 0 ? (
        <EmptyState
          title={t("dashboards.json.noEndpoints")}
          description={t("dashboards.json.noEndpointsHint")}
          action={
            <Link to="/projects/$project/$plural" params={{ project, plural: "endpoints" }}>
              {t("dashboards.json.toEndpoints")}
            </Link>
          }
        />
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          <section aria-labelledby={`${ids.editor}-title`} className="flex min-w-0 flex-col gap-3">
            <h2 id={`${ids.editor}-title`} className="text-title">
              {t("dashboards.json.source")}
            </h2>
            <Card padding="sm" className="flex flex-col gap-3">
              <div className="grid gap-3 sm:grid-cols-3">
                <Field id={ids.type} label={t("dashboards.json.widgetType")} help={t("dashboards.json.widgetTypeHint")}>
                  <Select id={ids.type} value={pickedType} onChange={(e) => setPickedType(e.target.value as PickedWidget)}>
                    {PICKED_WIDGETS.map((type) => (
                      <option key={type} value={type}>
                        {t(`dashboards.json.type.${type}`)}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field id={ids.endpoint} label={t("dashboards.json.endpoint")} help={t("dashboards.json.endpointHint")}>
                  <Select id={ids.endpoint} value={endpoint} onChange={(e) => setPickedEndpoint(e.target.value)}>
                    {endpointNames.map((e) => (
                      <option key={e} value={e}>
                        {e}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field
                  id={ids.entity}
                  label={t("dashboards.json.entityType")}
                  help={entityTypes.length > 0 ? t("dashboards.json.entityTypeHint") : t("dashboards.json.noTypes")}
                >
                  <Select id={ids.entity} value={entityType} disabled={entityTypes.length === 0} aria-describedby={`${ids.entity}__help`} onChange={(e) => setPickedEntity(e.target.value)}>
                    {entityTypes.map((type) => (
                      <option key={type} value={type}>
                        {type}
                      </option>
                    ))}
                  </Select>
                </Field>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="secondary"
                  disabled={!canInsert}
                  aria-describedby={ids.why}
                  onClick={() => {
                    const next = insertWidget(text, widgetSnippet(pickedType, endpoint, entityType));
                    if (next) {
                      setTyped(next);
                      setInserted(true);
                    }
                  }}
                >
                  {t("dashboards.json.insert")}
                </Button>
                {!parsed.manifest ? (
                  <span id={ids.why} className="text-caption text-fg-muted">{t("dashboards.json.fixFirst")}</span>
                ) : inserted ? (
                  <span role="status" className="text-caption text-fg-muted">
                    {t("dashboards.json.inserted")}
                  </span>
                ) : null}
              </div>
            </Card>
            <p id={`${ids.editor}-hint`} className="text-caption text-fg-muted">
              {t("dashboards.json.editorHint")}
            </p>
            <Card flush className="overflow-hidden">
              <Suspense fallback={<Skeleton className="h-112" />}>
                <DashboardJsonMonaco
                  value={text}
                  onChange={(next) => {
                    setTyped(next);
                    setInserted(false);
                    check.reset();
                  }}
                  schema={editorSchema}
                  ariaLabel={t("dashboards.json.source")}
                  height="min(50vh, 36rem)"
                />
              </Suspense>
            </Card>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="secondary"
                disabled={!parsed.manifest || check.isPending}
                aria-describedby={`${ids.editor}-next ${ids.editor}-state`}
                onClick={() => {
                  const body = asProposal();
                  if (body) check.mutate(body);
                }}
              >
                {t("dashboards.json.check")}
              </Button>
              <Button
                variant="primary"
                disabled={!parsed.manifest || proposal.mutation.isPending}
                aria-describedby={`${ids.editor}-next ${ids.editor}-state`}
                onClick={() => {
                  const body = asProposal();
                  if (body) proposal.mutation.mutate({ body, create: !existing });
                }}
              >
                {t("dashboards.propose")}
              </Button>
              <span id={`${ids.editor}-next`} className="text-caption text-fg-muted">
                {!parsed.manifest
                  ? t("dashboards.json.fixJson")
                  : refusals.length === 0 && !checkedGreen
                    ? t("dashboards.json.nextStep")
                    : null}
              </span>
            </div>
            <div id={`${ids.editor}-state`} className="flex flex-col gap-3 empty:hidden">
              {parsed.error ? (
                <Alert tone="danger" title={t("dashboards.json.invalid")}>
                  {parsed.error.line
                    ? t("dashboards.json.invalidAt", { line: parsed.error.line, column: parsed.error.column, message: parsed.error.message })
                    : parsed.error.message}
                </Alert>
              ) : null}
              {refusals.length > 0 ? (
                <Alert tone="danger" title={t("dashboards.json.refused")}>
                  <p>{t("dashboards.json.refusedHint")}</p>
                  <ul className="list-disc pl-5">
                    {refusals.map((finding, index) => (
                      <li key={`${finding.path ?? ""}-${index}`}>
                        {finding.path ? <code>{finding.path}</code> : null}
                        {finding.path ? ": " : null}
                        {finding.message}
                      </li>
                    ))}
                  </ul>
                </Alert>
              ) : checkedGreen ? (
                <Alert tone="success" title={t("dashboards.json.checked")}>
                  {t("dashboards.addHint")}
                </Alert>
              ) : null}
              {proposal.error ? <Alert tone="danger">{proposal.error}</Alert> : null}
            </div>
          </section>
          <section aria-labelledby={`${ids.editor}-preview`} aria-live="polite" className="flex min-w-0 flex-col gap-3">
            <h2 id={`${ids.editor}-preview`} className="text-title">
              {t("dashboards.json.preview")}
            </h2>
            {parsed.manifest && refusals.length > 0 ? (
              <Alert tone="warning">{t("dashboards.json.previewRefused")}</Alert>
            ) : null}
            {!parsed.manifest ? (
              <Card padding="sm">
                <p role="status" className="text-body text-fg-muted">
                  {t("dashboards.json.previewInvalid")}
                </p>
              </Card>
            ) : pages.every((page) => page.widgets.length === 0 && page.refused.length === 0) ? (
              <Card padding="sm">
                <p role="status" className="text-body text-fg-muted">
                  {t("dashboards.json.previewEmpty")}
                </p>
              </Card>
            ) : (
              pages.map((page) => (
                <div key={page.key} className="flex flex-col gap-3">
                  {page.title ? <h3 className="text-body font-medium">{page.title}</h3> : null}
                  {page.refused.length > 0 ? (
                    <Alert tone="warning">{t("dashboards.json.notPublic", { endpoints: page.refused.join(", ") })}</Alert>
                  ) : null}
                  <DashboardWidgets project={project} widgets={page.widgets} />
                </div>
              ))
            )}
          </section>
        </div>
      )}
    </div>
  );
}
