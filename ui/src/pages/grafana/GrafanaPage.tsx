import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, reasonOf, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Card,
  CardHeader,
  EmptyState,
  Field,
  InlineError,
  Input,
  PageFailed,
  PageHeader,
  PageLoading,
  Select,
  Skeleton,
  safeHref,
} from "../../components/ui";

type Item = components["schemas"]["VisualizationItem"];
type Kind = components["schemas"]["Kind"];
type Options = components["schemas"]["GrafanaOptions"];

const KINDS: readonly Kind[] = ["timeseries", "stat", "table", "map"];

const pageKey = (project: string) => ["projects", project, "grafana"];

function useOptions(project: string, endpoint: string, type: string) {
  return useQuery({
    queryKey: [...pageKey(project), "options", endpoint, type],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/grafana/options", {
          params: {
            path: { project },
            query: { endpoint: endpoint || undefined, type: type || undefined },
          },
        }),
      ),
  });
}

/** One saved visualization: what it draws, where it opens, and its removal, asked twice. */
export function SavedVisualization({
  project,
  item,
  mayRemove,
}: {
  project: string;
  item: Item;
  mayRemove: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState(false);
  const remove = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/grafana/{id}", {
          params: { path: { project, id: item.id } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: pageKey(project) }),
  });
  const safe = safeHref(item.openUrl);
  return (
    <li className="flex flex-wrap items-center gap-2 border-b border-border py-3 last:border-b-0">
      <div className="min-w-0 flex-1 basis-full sm:basis-60">
        <p className="break-words text-body font-medium text-fg">{item.title}</p>
        <p className="break-words text-caption text-fg-muted">
          {t("grafana.saved.reads", { endpoint: item.endpointTitle, type: item.type, attribute: item.attribute })}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Badge>{t(`grafana.kind.${item.kind}`)}</Badge>
        {safe ? (
          <a
            className={buttonClass("secondary", "sm")}
            href={safe}
            target="_blank"
            rel="noopener noreferrer"
          >
            <span aria-hidden="true">{t("grafana.saved.openShort")}</span>
            <span className="sr-only">{t("grafana.saved.open", { title: item.title })}</span>
          </a>
        ) : null}
        {mayRemove && !asking ? (
          <Button
            variant="ghost"
            size="sm"
            aria-label={t("grafana.saved.remove", { title: item.title })}
            onClick={() => setAsking(true)}
          >
            {t("grafana.saved.removeShort")}
          </Button>
        ) : null}
        {asking ? (
          <span className="flex items-center gap-2">
            <Button
              variant="danger"
              size="sm"
              disabled={remove.isPending}
              disabledReason={t("grafana.saved.removing")}
              onClick={() => remove.mutate()}
            >
              {t("grafana.saved.confirm")}
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setAsking(false)}>
              {t("grafana.saved.keep")}
            </Button>
          </span>
        ) : null}
      </div>
      {remove.isError ? (
        <Alert tone="danger" className="basis-full">
          {reasonOf(remove.error)}
        </Alert>
      ) : null}
    </li>
  );
}

/** A value as a line of text; a place by its first coordinates, never as raw JSON. */
function valueText(value: unknown): string {
  const coordinates = (value as { coordinates?: unknown } | null)?.coordinates;
  if (Array.isArray(coordinates)) return coordinates.flat(3).slice(0, 2).join(", ");
  if (value === undefined || value === null) return "–";
  return typeof value === "object" ? "…" : String(value);
}

interface PreviewRow {
  id: string;
  name?: unknown;
  [attribute: string]: unknown;
}

/**
 * The first rows the visualization will draw, read through the Endpoint with the person's own
 * session: what they would see in Grafana, refusals included.
 */
export function VisualizationPreview({
  slug,
  type,
  attribute,
}: {
  slug: string;
  type: string;
  attribute: string;
}): JSX.Element {
  const { t } = useTranslation();
  const rows = useQuery({
    queryKey: ["grafana-preview", slug, type, attribute],
    queryFn: async (): Promise<PreviewRow[]> => {
      const query = new URLSearchParams({ type, attrs: `name,${attribute}`, limit: "5", options: "keyValues" });
      const response = await fetch(`/api/endpoint/${slug}/ngsi-ld/v1/entities?${query}`, {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
      if (!response.ok) {
        const error: unknown = await response.json().catch(() => undefined);
        await unwrap({ error, response });
      }
      return (await response.json()) as PreviewRow[];
    },
  });
  return (
    <Card>
      <CardHeader as="h2" title={t("grafana.form.preview")} className="mb-2" />
      {rows.isPending ? <Skeleton className="h-24" /> : null}
      {rows.isError ? <InlineError>{reasonOf(rows.error, t("grafana.form.previewFailed"))}</InlineError> : null}
      {rows.isSuccess && rows.data.length === 0 ? (
        <p className="text-body text-fg-muted">{t("grafana.form.previewNoRows")}</p>
      ) : null}
      {rows.isSuccess && rows.data.length > 0 ? (
        <dl>
          {rows.data.map((row) => (
            <div key={row.id} className="flex justify-between gap-3 border-b border-border py-1.5 last:border-b-0">
              <dt className="min-w-0 break-words text-body text-fg-muted">
                {typeof row.name === "string" ? row.name : row.id.split(":").pop()}
              </dt>
              <dd className="text-body font-medium text-fg">{valueText(row[attribute])}</dd>
            </div>
          ))}
        </dl>
      ) : null}
    </Card>
  );
}

/** The kinds the chosen attribute can be drawn as, or all of them until one is chosen. */
function kindsFor(options: Options | undefined, attribute: string): readonly Kind[] {
  return options?.attributes.find((option) => option.name === attribute)?.kinds ?? KINDS;
}

function useChoices(project: string) {
  const [endpoint, setEndpoint] = useState("");
  const [type, setType] = useState("");
  const [attribute, setAttribute] = useState("");
  const [kind, setKind] = useState<Kind>("timeseries");
  const [title, setTitle] = useState("");
  const options = useOptions(project, endpoint, type);
  return {
    endpoint,
    type,
    attribute,
    kind,
    title,
    options,
    setTitle,
    setKind,
    chooseEndpoint: (next: string) => {
      setEndpoint(next);
      setType("");
      setAttribute("");
    },
    chooseType: (next: string) => {
      setType(next);
      setAttribute("");
    },
    chooseAttribute: (next: string) => {
      setAttribute(next);
      const fits = kindsFor(options.data, next);
      if (!fits.includes(kind) && fits[0]) setKind(fits[0]);
    },
  };
}

/**
 * The form. Each dropdown lists what the server offers this person; the server checks each value
 * again on the save, and a refusal names its field here. The preview reads through the Endpoint
 * with the person's own session, so it shows what they would see in Grafana.
 */
export function VisualizationForm({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const form = useChoices(project);
  const save = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/grafana", {
          params: { path: { project } },
          body: {
            title: form.title,
            endpoint: form.endpoint,
            type: form.type,
            attribute: form.attribute,
            kind: form.kind,
          },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: pageKey(project) }),
  });
  const endpoints = form.options.data?.endpoints ?? [];
  const usable = endpoints.filter((endpoint) => endpoint.usable);
  const closed = endpoints.filter((endpoint) => !endpoint.usable);
  const chosen = usable.find((endpoint) => endpoint.name === form.endpoint);
  const attributes = form.options.data?.attributes ?? [];
  const ready = Boolean(form.title.trim() && chosen && form.type && form.attribute);
  if (form.options.isError) {
    return <PageFailed error={form.options.error} />;
  }
  if (form.options.isSuccess && usable.length === 0) {
    return (
      <EmptyState
        icon="endpoints"
        title={t("grafana.form.noEndpointsTitle")}
        description={t("grafana.form.noEndpoints")}
      />
    );
  }
  const safe = safeHref(save.data?.openUrl);
  return (
    <div className="grid items-start gap-card lg:grid-cols-2 [&>*]:min-w-0">
      <Card>
        <CardHeader as="h2" title={t("grafana.form.title")} className="mb-3" />
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (ready) save.mutate();
          }}
        >
          <Field id="grafana-endpoint" label={t("grafana.form.endpoint")} help={t("grafana.form.endpointHelp")}>
            <Select id="grafana-endpoint" value={form.endpoint} onChange={(event) => form.chooseEndpoint(event.target.value)}>
              <option value="">{t("grafana.form.choose")}</option>
              {usable.map((endpoint) => (
                <option key={endpoint.name} value={endpoint.name}>
                  {endpoint.title}
                </option>
              ))}
            </Select>
          </Field>
          {closed.length > 0 ? (
            <p className="text-caption text-fg-muted">
              {t("grafana.form.notReachable", { names: closed.map((endpoint) => endpoint.title).join(", ") })}
            </p>
          ) : null}
          <Field id="grafana-type" label={t("grafana.form.type")} help={t("grafana.form.typeHelp")}>
            <Select
              id="grafana-type"
              value={form.type}
              disabled={!chosen}
              title={chosen ? undefined : t("grafana.form.chooseEndpointFirst")}
              onChange={(event) => form.chooseType(event.target.value)}
            >
              <option value="">{t("grafana.form.choose")}</option>
              {(form.options.data?.types ?? []).map((type) => (
                <option key={type} value={type}>
                  {type}
                </option>
              ))}
            </Select>
          </Field>
          <Field id="grafana-attribute" label={t("grafana.form.attribute")} help={t("grafana.form.attributeHelp")}>
            <Select
              id="grafana-attribute"
              value={form.attribute}
              disabled={!form.type}
              title={form.type ? undefined : t("grafana.form.chooseTypeFirst")}
              onChange={(event) => form.chooseAttribute(event.target.value)}
            >
              <option value="">{t("grafana.form.choose")}</option>
              {attributes.map((attribute) => (
                <option key={attribute.name} value={attribute.name}>
                  {attribute.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field id="grafana-kind" label={t("grafana.form.kind")} help={t("grafana.form.kindHelp")}>
            <Select id="grafana-kind" value={form.kind} onChange={(event) => form.setKind(event.target.value as Kind)}>
              {kindsFor(form.options.data, form.attribute).map((kind) => (
                <option key={kind} value={kind}>
                  {t(`grafana.kind.${kind}`)}
                </option>
              ))}
            </Select>
          </Field>
          <Field id="grafana-title" label={t("grafana.form.name")} help={t("grafana.form.nameHelp")}>
            <Input id="grafana-title" value={form.title} maxLength={120} onChange={(event) => form.setTitle(event.target.value)} />
          </Field>
          <div>
            <Button
              variant="primary"
              type="submit"
              disabled={!ready || save.isPending}
              disabledReason={ready ? t("grafana.form.saving") : t("grafana.form.incomplete")}
            >
              {t("grafana.form.save")}
            </Button>
          </div>
          {save.isError ? <Alert tone="danger">{reasonOf(save.error)}</Alert> : null}
          {save.isSuccess ? (
            <Alert tone="success">
              {t("grafana.form.saved", { title: save.data.title })}{" "}
              {safe ? (
                <a className="underline" href={safe} target="_blank" rel="noopener noreferrer">
                  {t("grafana.form.openSaved")}
                </a>
              ) : null}
            </Alert>
          ) : null}
        </form>
      </Card>
      {chosen && form.type && form.attribute ? (
        <VisualizationPreview slug={chosen.slug} type={form.type} attribute={form.attribute} />
      ) : (
        <Card>
          <EmptyState
            bare
            icon="dashboards"
            title={t("grafana.form.previewEmptyTitle")}
            description={t("grafana.form.previewEmpty")}
          />
        </Card>
      )}
    </div>
  );
}

/** The project's visualizations in Grafana, and the form that adds one (T-6509). */
export function GrafanaPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const page = useQuery({
    queryKey: pageKey(project),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/grafana", { params: { path: { project } } })),
  });
  const header = <PageHeader title={t("grafana.title")} description={t("grafana.lead")} />;
  if (page.isPending) {
    return <PageLoading label={t("grafana.title")} />;
  }
  if (page.isError) {
    return (
      <div className="flex flex-col gap-section">
        {header}
        <PageFailed error={page.error} />
      </div>
    );
  }
  const { ready, reason, mayCreate, items } = page.data;
  return (
    <div className="flex flex-col gap-section">
      {header}
      {ready ? null : (
        <EmptyState
          icon="dashboards"
          title={t("grafana.notReadyTitle")}
          description={reason ?? t("grafana.notReady")}
        />
      )}
      {ready && mayCreate ? <VisualizationForm project={project} /> : null}
      {ready && !mayCreate ? (
        <Card>
          <CardHeader as="h2" title={t("grafana.readOnlyTitle")} className="mb-1" />
          <p className="text-body text-fg-muted">{t("grafana.readOnly")}</p>
        </Card>
      ) : null}
      {ready ? (
        <Card>
          <CardHeader as="h2" title={t("grafana.saved.title")} className="mb-1" />
          {items.length === 0 ? (
            <EmptyState
              bare
              icon="dashboards"
              title={t("grafana.saved.emptyTitle")}
              description={t(mayCreate ? "grafana.saved.empty" : "grafana.saved.emptyReader")}
            />
          ) : (
            <ul>
              {items.map((item) => (
                <SavedVisualization key={item.id} project={project} item={item} mayRemove={mayCreate} />
              ))}
            </ul>
          )}
        </Card>
      ) : null}
    </div>
  );
}
