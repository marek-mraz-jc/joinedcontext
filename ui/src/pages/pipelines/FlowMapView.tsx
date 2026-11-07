import { useEffect, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { isChange } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { handPrefill } from "../../assistant/state";
import { useOrgDomain } from "../../api/projects";
import { ChangeNotice } from "../../components/ChangeNotice";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { Alert, Badge, Button, Card, EmptyState, Field, PageFailed, PageLoading, PermissionGuard, Select } from "../../components/ui";
import { proposeEnabled } from "./PipelineOverview";
import {
  CARD_HEIGHT,
  CARD_WIDTH,
  curve,
  groups,
  layout,
  missingGrants,
  neighbours,
  targets,
  targetUrn,
} from "./flowMap";
import type { FlowEdge, FlowMap, FlowNode } from "./flowMap";

function useFlowMap(project: string, space: string) {
  return useQuery({
    queryKey: [...queryKeys.list(project, "pipelines"), "flow-map", space],
    queryFn: async (): Promise<FlowMap> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/flow-map", {
          params: { path: { project }, query: space ? { space } : {} },
        }),
      ),
  });
}

/** The space the address names (`?space=`), so a space's page can open its own paths. */
function spaceFromAddress(): string {
  return typeof window === "undefined" ? "" : (new URLSearchParams(window.location.search).get("space") ?? "");
}

/**
 * The project's data flow map (T-4862): left to right, from MQTT broker and DataSource through
 * the pipelines and their Endpoints into the spaces, and on through derived and KPI pipelines.
 * Every read and write of a pipeline carries the Policy that grants it, a missing one in red with
 * the fix. A phone gets the same nodes as a grouped list.
 */
export function FlowMapView({
  project,
  onNewPipeline,
}: {
  project: string;
  /** Opens the pipeline editor on this form, on the page the map is part of. */
  onNewPipeline: (form: Record<string, unknown>) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [space, setSpace] = useState(spaceFromAddress);
  const [selected, setSelected] = useState<string | null>(null);
  const whole = useFlowMap(project, "");
  const shown = useFlowMap(project, space);
  const map = shown.data;

  const chooseSpace = (value: string) => {
    setSpace(value);
    setSelected(null);
    const url = new URL(window.location.href);
    if (value) {
      url.searchParams.set("space", value);
    } else {
      url.searchParams.delete("space");
    }
    window.history.replaceState(window.history.state, "", url);
  };

  const spaces = (whole.data?.nodes ?? []).filter((node) => node.kind === "ContextSpace" && node.project === project);
  const node = map?.nodes.find((candidate) => candidate.id === selected);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-end sm:justify-between">
        <Field id="flow-map-space" label={t("pipelines.map.space")} className="sm:w-72">
          <Select id="flow-map-space" value={space} onChange={(event) => chooseSpace(event.target.value)}>
            <option value="">{t("pipelines.map.allSpaces")}</option>
            {spaces.map((candidate) => (
              <option key={candidate.id} value={candidate.name}>
                {candidate.title ?? candidate.name}
              </option>
            ))}
          </Select>
        </Field>
        <Legend />
      </div>

      {shown.isPending ? (
        <PageLoading label={t("pipelines.map.title")} />
      ) : shown.isError ? (
        <PageFailed error={shown.error} onRetry={() => void shown.refetch()} />
      ) : map && map.nodes.length > 0 ? (
        <>
          <MissingGrants map={map} onSelect={setSelected} />
          <MapCanvas map={map} selected={selected} onSelect={setSelected} />
          <GroupedList map={map} selected={selected} onSelect={setSelected} />
          {node ? <NodeDetail project={project} map={map} node={node} onSelect={setSelected} onNewPipeline={onNewPipeline} /> : null}
        </>
      ) : (
        <EmptyState
          icon="pipelines"
          title={space ? t("pipelines.map.emptySpace") : t("pipelines.map.empty")}
          description={t("pipelines.map.emptyHint")}
        />
      )}
    </div>
  );
}

function Legend(): JSX.Element {
  const { t } = useTranslation();
  const item = (className: string, label: string) => (
    <li className="flex items-center gap-2">
      <svg aria-hidden="true" width="28" height="8">
        <line x1="0" y1="4" x2="28" y2="4" strokeWidth="2" className={className} />
      </svg>
      {label}
    </li>
  );
  return (
    <ul aria-label={t("pipelines.map.legend")} className="flex flex-wrap gap-4 text-caption text-fg-muted">
      {item("stroke-border-strong", t("pipelines.map.grant.granted"))}
      {item("stroke-danger", t("pipelines.map.grant.missing"))}
      {item("stroke-info", t("pipelines.map.grant.external"))}
    </ul>
  );
}

function MissingGrants({ map, onSelect }: { map: FlowMap; onSelect: (id: string) => void }): JSX.Element | null {
  const { t } = useTranslation();
  const missing = missingGrants(map);
  if (missing.length === 0) {
    return null;
  }
  return (
    <Alert tone="danger" role="status" title={t("pipelines.map.missingTitle", { count: missing.length })}>
      <ul className="flex flex-col gap-1">
        {missing.map((edge) => {
          const pipeline = edge.relation === "writes" ? edge.from : edge.to;
          return (
            <li key={`${edge.from}>${edge.to}`}>
              <Button variant="ghost" size="xs" className="underline" onClick={() => onSelect(pipeline)}>
                {nameOf(map, pipeline)}
              </Button>
              {": "}
              {edge.grant?.fix}
            </li>
          );
        })}
      </ul>
    </Alert>
  );
}

function nameOf(map: FlowMap, id: string): string {
  const node = map.nodes.find((candidate) => candidate.id === id);
  return node ? (node.title ?? node.name) : id;
}

/** The space behind an Endpoint node, or its project when the caller may not read it. */
function spaceOf(map: FlowMap, id: string): string {
  const node = map.nodes.find((candidate) => candidate.id === id);
  return node?.space ?? node?.project ?? id;
}

function edgeClass(edge: FlowEdge): string {
  switch (edge.grant?.state) {
    case "missing":
      return "stroke-danger";
    case "external":
      return "stroke-info";
    default:
      return "stroke-border-strong";
  }
}

/** The desktop map: cards in columns, the edges drawn under them. */
function MapCanvas({
  map,
  selected,
  onSelect,
}: {
  map: FlowMap;
  selected: string | null;
  onSelect: (id: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const placed = useMemo(() => layout(map), [map]);
  const at = new Map(placed.placed.map((entry) => [entry.node.id, entry]));
  return (
    <div className="hidden overflow-x-auto rounded-lg border border-border bg-surface-subtle p-6 sm:block">
      {/* One drawing: the edges under the cards, each card a button at its computed place. */}
      <svg role="group" aria-label={t("pipelines.map.title")} width={placed.width} height={placed.height}>
        <g aria-hidden="true">
          {map.edges.map((edge) => {
            const from = at.get(edge.from);
            const to = at.get(edge.to);
            return from && to ? (
              <path
                key={`${edge.from}>${edge.to}>${edge.relation}`}
                d={curve(from, to)}
                fill="none"
                strokeWidth={edge.grant?.state === "missing" ? 2.5 : 1.5}
                strokeDasharray={edge.grant?.state === "external" || edge.grant?.state === "missing" ? "6 4" : undefined}
                className={edgeClass(edge)}
              />
            ) : null;
          })}
        </g>
        {placed.placed.map((entry) => (
          <foreignObject key={entry.node.id} x={entry.x} y={entry.y} width={CARD_WIDTH} height={CARD_HEIGHT}>
            <NodeCard map={map} node={entry.node} selected={selected === entry.node.id} onSelect={onSelect} />
          </foreignObject>
        ))}
      </svg>
    </div>
  );
}

function NodeCard({
  map,
  node,
  selected,
  onSelect,
}: {
  map: FlowMap;
  node: FlowNode;
  selected: boolean;
  onSelect: (id: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const red = missingGrants(map, node.id).length > 0 && node.kind === "Pipeline";
  const kind = t(`pipelines.map.kind.${node.kind}`);
  const line = summary(t, node);
  return (
    <Button
      aria-pressed={selected}
      aria-label={`${kind}: ${node.title ?? node.name}${line ? `. ${line}` : ""}`}
      onClick={() => onSelect(node.id)}
      className={clsx(
        "h-full w-full flex-col items-stretch justify-center gap-0 py-1.5 text-left",
        red && "border-danger",
        selected && "ring-2 ring-primary",
        node.opaque && "border-dashed",
      )}
    >
      <span className="shrink-0 truncate text-caption text-fg-subtle">
        {kind}
        {node.role ? ` · ${t(`pipelines.map.role.${node.role}`)}` : ""}
        {node.project !== map.project ? ` · ${node.project}` : ""}
      </span>
      <span className="shrink-0 truncate font-medium text-fg">{node.title ?? node.name}</span>
      <span className="shrink-0 truncate text-caption text-fg-muted">{line}</span>
    </Button>
  );
}

type T = ReturnType<typeof useTranslation>["t"];

/** One line under a node's name: what a person checks first. */
function summary(t: T, node: FlowNode): string {
  switch (node.kind) {
    case "Pipeline": {
      const state = node.enabled === false ? t("pipelines.map.paused") : (node.phase ?? t("pipelines.map.noPhase"));
      const run = node.lastRun;
      return run ? `${state} · ${t("pipelines.map.sent", { count: run.sent })}` : state;
    }
    case "DataSource":
      return [node.sourceType, node.hosts?.[0]].filter(Boolean).join(" · ");
    case "Endpoint":
      return node.opaque ? t("pipelines.map.opaque") : (node.space ?? "");
    default:
      return node.name;
  }
}

/** The phone's map: every node in its group, with what it reads from and writes to. */
function GroupedList({
  map,
  selected,
  onSelect,
}: {
  map: FlowMap;
  selected: string | null;
  onSelect: (id: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-4 sm:hidden">
      {groups(map).map(({ group, nodes }) => (
        <section key={group} aria-labelledby={`flow-group-${group}`} className="flex flex-col gap-2">
          <h2 id={`flow-group-${group}`} className="text-sm font-semibold text-fg">
            {t(`pipelines.map.group.${group}`)}
          </h2>
          <ul className="flex flex-col gap-2">
            {nodes.map((node) => (
              <li key={node.id} className="flex flex-col gap-1">
                <NodeCard map={map} node={node} selected={selected === node.id} onSelect={onSelect} />
                <Connections map={map} node={node} />
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function Connections({ map, node }: { map: FlowMap; node: FlowNode }): JSX.Element | null {
  const { t } = useTranslation();
  const { incoming, outgoing } = neighbours(map, node.id);
  if (incoming.length + outgoing.length === 0) {
    return null;
  }
  const line = (edge: FlowEdge, other: string, key: "from" | "to") => (
    <li key={`${key}-${other}`} className={clsx(edge.grant?.state === "missing" && "text-danger")}>
      {t(`pipelines.map.${key}`, { name: nameOf(map, other) })}
      {edge.grant ? ` · ${t(`pipelines.map.grant.${edge.grant.state}`)}` : ""}
    </li>
  );
  return (
    <ul className="px-3 text-caption text-fg-muted">
      {incoming.map((edge) => line(edge, edge.from, "from"))}
      {outgoing.map((edge) => line(edge, edge.to, "to"))}
    </ul>
  );
}

/** What one node is and what a person can do with it from the map. */
function NodeDetail({
  project,
  map,
  node,
  onSelect,
  onNewPipeline,
}: {
  project: string;
  map: FlowMap;
  node: FlowNode;
  onSelect: (id: string | null) => void;
  onNewPipeline: (form: Record<string, unknown>) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus(), [node.id]);
  const { incoming, outgoing } = neighbours(map, node.id);
  const grants = [...incoming, ...outgoing].filter((edge) => edge.grant);
  return (
    <Card className="flex flex-col gap-4" aria-labelledby="flow-node-heading">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="flex flex-col gap-1">
          <span className="text-caption text-fg-subtle">{t(`pipelines.map.kind.${node.kind}`)}</span>
          <h2 id="flow-node-heading" ref={heading} tabIndex={-1} className="text-lg font-semibold text-fg">
            {node.title ?? node.name}
          </h2>
        </div>
        <Button size="sm" onClick={() => onSelect(null)}>
          {t("pipelines.map.close")}
        </Button>
      </div>
      <Facts node={node} />
      {grants.length > 0 ? <Grants map={map} edges={grants} /> : null}
      <Actions project={project} map={map} node={node} onNewPipeline={onNewPipeline} />
    </Card>
  );
}

function Facts({ node }: { node: FlowNode }): JSX.Element {
  const { t, i18n } = useTranslation();
  const rows: [string, React.ReactNode][] = [];
  const add = (key: string, value: React.ReactNode | undefined) => {
    if (value !== undefined && value !== "" && value !== null) {
      rows.push([t(`pipelines.map.fact.${key}`), value]);
    }
  };
  add("name", <span className="font-mono">{node.name}</span>);
  if (node.opaque) {
    add("owner", t("pipelines.map.opaqueOwner", { project: node.project }));
  }
  add("sourceType", node.sourceType);
  add("hosts", node.hosts?.join(", "));
  add("topics", node.topics?.join(", "));
  add("space", node.space);
  add("slug", node.slug);
  add("audience", node.audience);
  if (node.kind === "Pipeline") {
    add(
      "state",
      <LifecycleBadge kind="phase" value={node.enabled === false ? "paused" : (node.phase ?? undefined)} />,
    );
  }
  const run = node.lastRun;
  if (run) {
    add("lastRun", new Date(run.at).toLocaleString(i18n.language));
    add("counts", t("pipelines.map.counts", { sent: run.sent, rejected: run.rejected, failed: run.failed }));
    add("rate", run.perMinute !== undefined && run.perMinute !== null ? t("pipelines.map.perMinute", { rate: run.perMinute }) : undefined);
  }
  return (
    <dl className="grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-[max-content_1fr]">
      {rows.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-caption text-fg-subtle">{label}</dt>
          <dd className="break-words text-sm text-fg">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function Grants({ map, edges }: { map: FlowMap; edges: FlowEdge[] }): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="flow-node-grants" className="flex flex-col gap-2">
      <h3 id="flow-node-grants" className="text-sm font-semibold text-fg">
        {t("pipelines.map.grants")}
      </h3>
      <ul className="flex flex-col gap-2">
        {edges.map((edge) => {
          const grant = edge.grant!;
          const tone = grant.state === "granted" ? "success" : grant.state === "missing" ? "danger" : "info";
          return (
            <li key={`${edge.from}>${edge.to}`} className="flex flex-col gap-1 text-sm">
              <span className="flex flex-wrap items-center gap-2">
                <Badge tone={tone}>{t(`pipelines.map.grant.${grant.state}`)}</Badge>
                {t(`pipelines.map.edge.${edge.relation}`, {
                  endpoint: grant.endpoint,
                  operation: grant.operation,
                  other: spaceOf(map, edge.relation === "writes" ? edge.to : edge.from),
                })}
              </span>
              {grant.policy ? (
                <span className="text-caption text-fg-muted">{t("pipelines.map.byPolicy", { policy: grant.policy })}</span>
              ) : null}
              {grant.fix ? <span className="text-caption text-danger">{t("pipelines.map.fix", { fix: grant.fix })}</span> : null}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function Actions({
  project,
  map,
  node,
  onNewPipeline,
}: {
  project: string;
  map: FlowMap;
  node: FlowNode;
  onNewPipeline: (form: Record<string, unknown>) => void;
}): JSX.Element | null {
  if (node.opaque || node.project !== project) {
    return null;
  }
  switch (node.kind) {
    case "Pipeline":
      return <PipelineActions project={project} node={node} />;
    case "DataSource":
    case "MqttBroker":
      return <SendToSpace project={project} map={map} node={node} onNewPipeline={onNewPipeline} />;
    case "Endpoint":
      return <OpenLink href={`/projects/${project}/endpoints/${node.name}`} labelKey="pipelines.map.openEndpoint" />;
    default:
      return <OpenLink href={`/projects/${project}/spaces/${node.name}`} labelKey="pipelines.map.openSpace" />;
  }
}

function OpenLink({ href, labelKey }: { href: string; labelKey: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  return (
    <div className="flex flex-wrap gap-2">
      <Button variant="primary" onClick={() => void navigate({ href })}>
        {t(labelKey)}
      </Button>
    </div>
  );
}

export function PipelineActions({ project, node }: { project: string; node: FlowNode }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  const page = `/projects/${project}/pipelines/${node.name}`;
  const running = node.enabled !== false;
  const toggle = useMutation({
    mutationFn: async () => {
      setError(null);
      const manifest = (await unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "pipelines", name: node.name } },
        }),
      )) as Manifest;
      return proposeEnabled(project, manifest, !running);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "pipelines") });
    },
    onError: (err) => setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic")),
  });
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" onClick={() => void navigate({ href: page })}>
          {t("pipelines.map.openPipeline")}
        </Button>
        <Button onClick={() => void navigate({ href: `${page}#runs` })}>{t("pipelines.map.openRuns")}</Button>
        <Button onClick={() => void navigate({ href: `${page}#rejected` })}>{t("pipelines.map.openRejected")}</Button>
        <PermissionGuard project={project} kind="Pipeline" verb="propose">
          <Button onClick={() => void navigate({ href: `${page}/edit` })}>{t("pipelines.map.edit")}</Button>
        </PermissionGuard>
        <PermissionGuard project={project} kind="Pipeline" verb="propose">
          <Button
            disabled={toggle.isPending}
            title={t(running ? "pipelines.pauseHint" : "pipelines.resumeHint")}
            onClick={() => toggle.mutate()}
          >
            {t(running ? "pipelines.pause" : "pipelines.resume")}
          </Button>
        </PermissionGuard>
      </div>
      {change ? <ChangeNotice change={change} project={project} /> : null}
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}
    </div>
  );
}

/**
 * "Send to a space": a DataSource opens the pipeline editor with itself as the source and the
 * chosen Endpoint as the target; a broker with no DataSource yet opens the DataSource form on it
 * first (PL-76), since a pipeline reads a broker through one.
 */
export function SendToSpace({
  project,
  map,
  node,
  onNewPipeline,
}: {
  project: string;
  map: FlowMap;
  node: FlowNode;
  onNewPipeline: (form: Record<string, unknown>) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const orgDomain = useOrgDomain(project);
  const endpoints = targets(map);
  const [target, setTarget] = useState(endpoints[0]?.id ?? "");
  const source =
    node.kind === "DataSource"
      ? node
      : map.nodes.find((other) => other.kind === "DataSource" && map.edges.some((e) => e.from === node.id && e.to === other.id));

  if (!source) {
    const datasources = `/projects/${project}/datasources`;
    return (
      <PermissionGuard project={project} kind="DataSource" verb="propose">
        <div className="flex flex-col gap-2">
          <p className="text-sm text-fg-muted">{t("pipelines.map.brokerFirst")}</p>
          <div>
            <Button
              variant="primary"
              onClick={() => {
                handPrefill(datasources, { type: "mqtt", mqtt: { brokerRef: { kind: "MqttBroker", name: node.name } } });
                void navigate({ href: datasources });
              }}
            >
              {t("pipelines.map.newDataSource")}
            </Button>
          </div>
        </div>
      </PermissionGuard>
    );
  }

  const send = () => {
    const endpoint = endpoints.find((candidate) => candidate.id === target);
    onNewPipeline({
      class: "auto",
      source: { dataSourceRef: source.name },
      ...(endpoint ? { targetEndpoint: targetUrn(orgDomain, endpoint) } : {}),
    });
  };
  return (
    <PermissionGuard project={project} kind="Pipeline" verb="propose">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
        <Field id="flow-send-target" label={t("pipelines.map.sendTarget")} className="sm:w-80">
          <Select id="flow-send-target" value={target} onChange={(event) => setTarget(event.target.value)}>
            <option value="">{t("pipelines.map.sendTargetLater")}</option>
            {endpoints.map((endpoint) => (
              <option key={endpoint.id} value={endpoint.id}>
                {t("pipelines.map.sendTargetOption", { endpoint: endpoint.title ?? endpoint.name, space: endpoint.space })}
              </option>
            ))}
          </Select>
        </Field>
        <Button variant="primary" onClick={send}>
          {t("pipelines.map.send")}
        </Button>
      </div>
    </PermissionGuard>
  );
}
