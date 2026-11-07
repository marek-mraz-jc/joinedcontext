import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useBranding } from "../../branding";
import { AccessPanel, useAccess } from "../../components/entities/AccessPanel";
import { deniedOf, useAttributeAccess } from "../../api/access";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { fetchEntities, filterSlotsOf, useModelSource } from "../../components/entities/filters";
import type { Entity, EntityQuery, FilterSlot } from "../../components/entities/filters";
import { RecordView,
  Alert,
  Button,
  Checkbox,
  Field,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Textarea,
} from "../../components/ui";
import { entityTypesOf, pickReadEndpoint, spaceOf } from "../spaces/spaceFacts";
import type { PipelineForm, RouteForm } from "./PipelineEditor";
import { endpointUrn } from "./PipelineEditor";
import { kpiTargets } from "./kpiEditor";
import { pathSegment } from "./outputMapper";
import { endpointUrl } from "../../components/endpoints/links";
import {
  OutputBlock,
  RouteBlock,
  removeRoute,
  removeRouteNode,
  routeNodeOf,
  PipelineFlow,
  countersOfStats,
  liveLabelOf,
  SourceBlock,
  StepBlock,
  outputIndexOf,
  removeOutput,
  removeSource,
  removeStep,
  samplesOf,
  setComputeKind,
  sourceIndexOf,
  stepIndexOf,
  toFlow,
} from "./PipelineFlow";
import type { FlowNodeId } from "./PipelineFlow";
import { PipelineTest, sourceBodyOf } from "./PipelineTest";
import type { Trace } from "./PipelineTest";
import { testPipeline } from "../../api/pipelineTest";
import { DebugSidebar, NodeDebug } from "./DebugPanel";
import { LastSamples } from "./LastSamples";
import type { DebugEntry } from "./DebugPanel";
import { FormHeading } from "../../components/forms/FormRoute";
import { AttributePicker } from "../../components/pickers/AttributePicker";
import { Combobox } from "../../components/pickers/Combobox";
import { TypePicker } from "../../components/pickers/TypePicker";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";

/** How many rows one sample shows: enough to tick a handful, small enough to read. */
export const SAMPLE_LIMIT = 20;
/** A page read once an hour when the author has not said otherwise: 3600 s is `scheduled`. */
const DEFAULT_PERIOD = "1h";

export type SourceKind = "none" | "datasource" | "space";
export type Aggregate = "sum" | "average" | "count";
const AGGREGATES: Aggregate[] = ["sum", "average", "count"];

/** One row of a keyValues sample: the id plus whatever attributes the entity carries. */
export type SampleRow = Entity;

export type StudioPreset = "load" | "kpi";

/** Converts a duration like "15m", "1h", "30s" to an ISO 8601 duration string. */
export function toIsoDuration(period: string): string {
  const trimmed = period.trim();
  if (trimmed.startsWith("P")) {
    return trimmed;
  }
  const match = /^(\d+)(ms|s|m|h|d)$/i.exec(trimmed);
  if (!match) {
    return "PT15M";
  }
  const count = match[1];
  const unit = match[2].toLowerCase();
  switch (unit) {
    case "s":
      return `PT${count}S`;
    case "m":
      return `PT${count}M`;
    case "h":
      return `PT${count}H`;
    case "d":
      return `P${count}D`;
    default:
      return "PT15M";
  }
}

/** Generates standard KPI indicator Bloblang conforming to Architecture/08. */
export function kpiBloblang(params: {
  kpiName: string;
  project: string;
  sourceEndpoint: string;
  sourceSpace: string;
  type: string;
  attribute: string;
  aggregate: "average" | "sum" | "count";
  period: string;
}): string {
  const { kpiName, project, sourceEndpoint, sourceSpace, type, attribute, aggregate, period } =
    params;
  const isoDuration = toIsoDuration(period);
  const formula =
    aggregate === "count"
      ? `count(${type})`
      : `${aggregate === "average" ? "avg" : "sum"}(${attribute}) over ${type}`;

  const valueExpr =
    aggregate === "count"
      ? "this.length()"
      : `if $stations.length() == 0 { 0 } else { $stations.map_each(s -> s.${pathSegment(attribute)}.value).sum() / $stations.length() }`;

  const filterLine =
    aggregate === "count"
      ? ""
      : `let stations = this.filter(s -> s.${pathSegment(attribute)}.value.type() == "number")\n`;

  return [
    `let domain = env("JC_ORG_DOMAIN")`,
    filterLine.trimEnd(),
    // Whole seconds: the broker refuses an observedAt with a nanosecond fraction, and a 15-minute
    // indicator does not need one (Architecture/08).
    `let now = now().ts_format("2006-01-02T15:04:05Z")`,
    `root.id = "urn:ngsi-ld:KeyPerformanceIndicator:%v:${project}-kpi:${kpiName}".format($domain)`,
    `root.type = "KeyPerformanceIndicator"`,
    `root.name = { "type": "Property", "value": ${JSON.stringify(kpiName)} }`,
    `root.calculationFormula = { "type": "Property", "value": ${JSON.stringify(formula)} }`,
    `root.currentValue = {`,
    `  "type": "Property",`,
    `  "value": ${valueExpr},`,
    `  "unitCode": "C62",`,
    `  "observedAt": $now`,
    `}`,
    `root.calculationPeriod = { "type": "Property", "value": { "start": $now.ts_sub_iso8601("${isoDuration}"), "end": $now } }`,
    `root.updatedAt = { "type": "Property", "value": { "@type": "DateTime", "@value": $now } }`,
    `root.derivedFrom = { "type": "Property", "value": "urn:ngsi-ld:Endpoint:%v:${sourceSpace}:${sourceEndpoint}".format($domain) }`,
    `root.computedBy = { "type": "Property", "value": "urn:ngsi-ld:Pipeline:%v:${project}:${kpiName}".format($domain) }`,
    "",
  ]
    .filter((line) => line !== "")
    .join("\n");
}

/** Finds the first Endpoint targeting a KPI space (`{project}-kpi`) to write indicators to. */
export function findKpiTargetEndpoint(
  project: string,
  endpoints: Manifest[],
  orgDomain?: string,
): string | undefined {
  // The one target list the KPI editor offers: nothing before the organization's domain is known,
  // never a made-up one written into the manifest (F-R2UI4-005).
  const targets = kpiTargets(endpoints, spaceOf, orgDomain);
  return (targets.find((target) => target.urn.includes(`:${project}-kpi:`)) ?? targets[0])?.urn;
}

/** Where the form's source points: a feed, a space, or nowhere yet. */
export function sourceKindOf(form: PipelineForm | undefined): SourceKind {
  if (form?.source?.dataSourceRef) {
    return "datasource";
  }
  if (form?.source?.endpointRef) {
    return "space";
  }
  return "none";
}

/** The attributes of one class of an inline LinkML model, `[]` when the model is not inline. */
export function attributesOf(model: Manifest | undefined, type: string | undefined): string[] {
  return filterSlotsOf(model, type).map((slot) => slot.name);
}

/**
 * The Bloblang of one aggregate over the page the fetch returns (PL-41): the page is one
 * message holding the entities, so the mapping folds it into one derived entity and hands the
 * output writer an array, the shape `entityOperations/upsert` takes.
 */
export function aggregateBloblang(
  aggregate: Aggregate,
  input: { type: string; attribute: string; space: string; outputType: string },
): string {
  const { type, attribute, space, outputType } = input;
  // Every name is data, from a model or a sample's keys: a path segment or a string literal,
  // never code (F-R2UI4-007).
  const values = `this.map_each(e -> e.${pathSegment(attribute)}.value.number().catch(0))`;
  const value = {
    sum: `${values}.sum()`,
    average: `if this.length() == 0 { 0 } else { ${values}.sum() / this.length() }`,
    count: "this.length()",
  }[aggregate];
  const property = `${attribute}${aggregate[0].toUpperCase()}${aggregate.slice(1)}`;
  return [
    `# ${aggregate} of ${JSON.stringify(attribute)} over the ${JSON.stringify(type)} entities the fetch returned (PL-41, PL-36)`,
    `let domain = env("JC_ORG_DOMAIN")`,
    `let seen = now().ts_format("2006-01-02T15:04:05Z")`,
    `root = [{`,
    `  "id": "urn:ngsi-ld:%v:%v:%v:%v".format(${JSON.stringify(outputType)}, $domain, ${JSON.stringify(space)}, ${JSON.stringify(`${attribute}-${aggregate}`)}),`,
    `  "type": ${JSON.stringify(outputType)},`,
    `  ${JSON.stringify(property)}: { "type": "Property", "value": ${value}, "observedAt": $seen },`,
    `  "derivedFrom": { "type": "Property", "value": this.map_each(e -> e.id) }`,
    `}]`,
    "",
  ].join("\n");
}

/**
 * A record's value as a person reads it in a cell (T-4972): text as it is, a nested object as
 * `key: value` pairs and a list as its items, never JSON (an XML source's root holds its
 * namespaces, `{"-gml": "http://…"}`).
 */
export function plainValue(value: unknown, depth = 0): string {
  if (typeof value === "string") return value;
  if (value === undefined) return "";
  if (value === null || typeof value !== "object") return String(value);
  if (depth >= 3) return "…";
  if (Array.isArray(value)) return value.map((item) => plainValue(item, depth + 1)).join(", ");
  const pairs = Object.entries(value).map(([key, member]) => `${key}: ${plainValue(member, depth + 1)}`).join("; ");
  return depth === 0 ? pairs : `(${pairs})`;
}

function cell(value: unknown): string {
  const text = plainValue(value);
  return text.length > 32 ? `${text.slice(0, 31)}…` : text;
}

export interface PipelineStudioProps {
  project: string;
  draft: PipelineForm | undefined;
  onChange: (form: PipelineForm) => void;
  dataSources: Manifest[];
  endpoints: Manifest[];
  /** The manifest the form is right now, for the sample test (PL-43); none hides the test. */
  toManifest?: (form: PipelineForm) => unknown;
  /** The test's answer for the mapping it ran (PL-49). */
  onVerdict?: (ok: boolean, bloblang: string) => void;
}

/** The URL of the `http` DataSource the draft reads, if it reads one (PL-48). */
export function sampleUrlOf(
  draft: PipelineForm | undefined,
  dataSources: Manifest[],
  endpoints: Manifest[] = [],
): string | undefined {
  const name = draft?.source?.dataSourceRef;
  const source = name
    ? dataSources.find((candidate) => candidate.metadata.name === name)
    : undefined;
  if (source) {
    if (source.spec.type !== "http") {
      return undefined;
    }
    const url = (source.spec.http as { url?: unknown } | undefined)?.url;
    return typeof url === "string" && /^https?:\/\//.test(url) ? url : undefined;
  }
  // An endpoint-sourced pipeline tests on a page of that endpoint, read the way the
  // reconciler will read it (PL-45): the runner fetches the URL under its own policy.
  const endpointName = draft?.source?.endpointRef;
  const endpoint = endpointName
    ? endpoints.find((candidate) => candidate.metadata.name === endpointName)
    : undefined;
  const slug = (endpoint?.spec as { slug?: unknown } | undefined)?.slug;
  const type = draft?.source?.query?.type;
  if (typeof slug !== "string" || !slug || typeof type !== "string" || !type) {
    return undefined;
  }
  const params = new URLSearchParams({ type, limit: "1000" });
  const attrs = draft?.source?.query?.attrs;
  if (Array.isArray(attrs) && attrs.length > 0) {
    params.set("attrs", attrs.join(","));
  }
  return endpointUrl(slug, `/ngsi-ld/v1/entities?${params.toString()}`);
}

/**
 * The studio above the pipeline form (UI-32): source, entities with a live sample to tick,
 * and an aggregate snippet. Every choice is written into the same form the fields below
 * and the YAML view edit, so nothing here is state of its own except the sample.
 */
export function PipelineStudio({
  project,
  draft,
  onChange,
  dataSources,
  endpoints,
  toManifest,
  onVerdict,
}: PipelineStudioProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const { orgDomain } = useBranding();
  const [sample, setSample] = useState<SampleRow[] | null>(null);
  const [sampleError, setSampleError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [aggregateAttribute, setAggregateAttribute] = useState("");
  const [studioView, setStudioView] = useState<"flow" | "form">("flow");
  // Opening a node (a double-click, Enter, Open in the list; PL-68) takes the person to its form:
  // the node's own editor under the canvas, or the source section for the first source.
  const [opening, setOpening] = useState<{ id: FlowNodeId; count: number } | null>(null);
  useEffect(() => {
    if (opening === null) return;
    const editor =
      document.querySelector<HTMLElement>('[data-testid^="flow-node-editor"]') ??
      (opening.id === "source" ? document.getElementById("studio-source-kind") : null);
    editor?.scrollIntoView?.({ block: "nearest" });
    (editor?.matches("input, select, textarea")
      ? editor
      : editor?.querySelector<HTMLElement>("input, select, textarea, button")
    )?.focus();
  }, [opening]);
  const [selectedNode, setSelectedNode] = useState<FlowNodeId | null>(() =>
    draft?.compute?.kind ? "compute" : "source",
  );
  const [flowTrace, setFlowTrace] = useState<Trace | null>(null);
  // The wires a Debug node taps and what each run sent across them (T-3222); a debug aid of the
  // studio, never part of the manifest.
  const [taps, setTaps] = useState<string[]>([]);
  const [debugLog, setDebugLog] = useState<DebugEntry[]>([]);
  // The running stream's counts per node and minute (PL-66, PL-68), for a pipeline that
  // exists: a new or paused one paints nothing live, a runner that does not answer says so.
  // Every five seconds while the tab is visible; a hidden tab asks nothing.
  const pipelineName = draft?.name ?? "";
  const liveStats = useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", pipelineName), "stats"],
    enabled: pipelineName !== "",
    retry: false,
    refetchInterval: 5_000,
    refetchIntervalInBackground: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/stats", {
          params: { path: { project, name: pipelineName } },
        }),
      ),
  });
  const liveCounters = liveStats.data?.nodes ? countersOfStats(liveStats.data.nodes) : undefined;
  // The queue on the wire into the output (T-3973), from the runner's metrics.
  const bufferQuery = useQuery({
    // Its own key: the pipelines list caches this route under `"metrics"` with a derived `rate`,
    // and a shared entry without it crashed that list's row (T-3090).
    queryKey: [...queryKeys.resource(project, "pipelines", pipelineName), "metrics", "nodes"],
    enabled: pipelineName !== "",
    retry: false,
    refetchInterval: 15_000,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/metrics", {
          params: { path: { project, name: pipelineName } },
        }),
      ),
  });
  const statsUnavailable = liveStats.error instanceof ApiError && liveStats.error.status === 503;
  // A failing node's last refused record, by the step it failed at: its message only, never
  // the record (PL-52).
  const failingLabel =
    selectedNode && (liveCounters?.[liveLabelOf(selectedNode)]?.errors ?? 0) > 0
      ? liveLabelOf(selectedNode)
      : undefined;
  const lastError = useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", pipelineName), "rejected", "last"],
    enabled: failingLabel !== undefined && !flowTrace,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/rejected", {
          params: { path: { project, name: pipelineName }, query: { limit: 20 } },
        }),
      ),
    select: (page) => page.items,
  });
  const failingStep = failingLabel?.startsWith("step_") ? Number(failingLabel.slice(5)) : undefined;
  const lastErrorOfNode = failingLabel
    ? lastError.data?.find((item) => (item.step ?? undefined) === failingStep)
    : undefined;
  const selectedStep = draft?.processors?.[stepIndexOf(selectedNode) ?? -1];
  const selectedSource = draft?.moreSources?.[sourceIndexOf(selectedNode) ?? -1];
  // The kind and the space are the author's choice until the form carries them: a chosen kind
  // with nothing picked yet, or a space with no endpoint, is not in the manifest at all.
  const [kindChoice, setKindChoice] = useState<SourceKind>(() => sourceKindOf(draft));
  const [spaceChoice, setSpaceChoice] = useState<string | undefined>(undefined);

  const [preset, setPreset] = useState<StudioPreset>(() =>
    draft?.output?.type === "KeyPerformanceIndicator" ? "kpi" : "load",
  );

  const draftEndpointName =
    typeof draft?.source?.endpointRef === "object"
      ? ((draft.source.endpointRef as { name?: string })?.name ?? "")
      : (draft?.source?.endpointRef ?? "");

  const [kpiEndpoint, setKpiEndpoint] = useState<string>(draftEndpointName);
  const [kpiType, setKpiType] = useState<string>(
    (draft?.source?.query?.type as string) || "BikeHireDockingStation",
  );
  const [kpiAttribute, setKpiAttribute] = useState<string>(
    (draft?.source?.query?.attrs as string[])?.[0] || "availableBikeNumber",
  );
  const [kpiAggregate, setKpiAggregate] = useState<"average" | "sum" | "count">("average");
  const [kpiPeriod, setKpiPeriod] = useState<string>(draft?.period || "15m");
  const [kpiName, setKpiName] = useState<string>(draft?.name || "bikes-available-avg");
  const [kpiTesting, setKpiTesting] = useState(false);
  const [kpiTestError, setKpiTestError] = useState<string | null>(null);
  const [kpiValue, setKpiValue] = useState<unknown | null>(null);

  function emitKpi(params: {
    endpointName: string;
    type: string;
    attribute: string;
    aggregate: "average" | "sum" | "count";
    period: string;
    name: string;
  }) {
    const ep = endpoints.find((e) => e.metadata.name === params.endpointName);
    const sourceSpace = (ep ? spaceOf(ep) : undefined) ?? project;
    const bloblang = kpiBloblang({
      kpiName: params.name,
      project,
      sourceEndpoint: params.endpointName,
      sourceSpace,
      type: params.type,
      attribute: params.attribute,
      aggregate: params.aggregate,
      period: params.period,
    });
    const targetEndpoint =
      findKpiTargetEndpoint(project, endpoints, orgDomain) ?? draft?.targetEndpoint;
    onChange({
      ...draft,
      name: params.name,
      // The reconciler picks the class from the cadence (PL-26): a period alone is a resident
      // stream that polls, and `scheduled` would demand a cron schedule the preset has not got.
      class: "auto",
      period: params.period,
      source: {
        ...draft?.source,
        endpointRef: params.endpointName,
        query: { type: params.type, attrs: [params.attribute] },
      },
      compute: {
        kind: "bloblang",
        bloblang,
      },
      output: {
        type: "KeyPerformanceIndicator",
        mode: "upsert",
      },
      targetEndpoint,
    });
  }

  async function runKpiTest() {
    if (!kpiEndpoint) {
      return;
    }
    const chosenEndpoint = endpoints.find((e) => e.metadata.name === kpiEndpoint);
    const slug =
      typeof chosenEndpoint?.spec.slug === "string" ? (chosenEndpoint.spec.slug as string) : "";
    if (!slug) {
      setKpiTestError(t("pipelines.test.failed", { status: 0 }));
      return;
    }

    const sourceSpace = (chosenEndpoint ? spaceOf(chosenEndpoint) : undefined) ?? project;
    // The mapping the proposal will carry, not a fresh rendering of it: a draft written
    // elsewhere — by the assistant, or typed in the YAML view — is what has to go green, or the
    // verdict belongs to a text nobody proposes and Propose never opens (PL-49, T-0911).
    const drafted = draft?.compute?.kind === "bloblang" ? (draft.compute.bloblang ?? "") : "";
    const bloblang =
      drafted.trim() !== ""
        ? drafted
        : kpiBloblang({
            kpiName,
            project,
            sourceEndpoint: kpiEndpoint,
            sourceSpace,
            type: kpiType,
            attribute: kpiAttribute,
            aggregate: kpiAggregate,
            period: kpiPeriod,
          });
    const targetEndpoint =
      findKpiTargetEndpoint(project, endpoints, orgDomain) ?? draft?.targetEndpoint;

    const kpiForm: PipelineForm = {
      ...draft,
      name: kpiName,
      class: "auto",
      period: kpiPeriod,
      source: {
        ...draft?.source,
        endpointRef: kpiEndpoint,
        query: { type: kpiType, attrs: [kpiAttribute] },
      },
      compute: {
        kind: "bloblang",
        bloblang,
      },
      output: {
        type: "KeyPerformanceIndicator",
        mode: "upsert",
      },
      targetEndpoint,
    };

    setKpiTesting(true);
    setKpiTestError(null);
    try {
      const url = endpointUrl(slug, `/ngsi-ld/v1/entities?${new URLSearchParams({ type: kpiType, attrs: kpiAttribute, limit: "1000" }).toString()}`);
      const answered = await testPipeline(project, toManifest ? toManifest(kpiForm) : kpiForm, {
        url,
        format: "json",
      });
      if (!("trace" in answered)) {
        setKpiTestError(answered.detail ?? t("pipelines.test.failed", { status: answered.status }));
        return;
      }
      const answer = answered.trace;
      // A trace that carries errors has no value to show: the first error is the answer.
      if (answer.errors.length > 0) {
        const first = answer.errors[0];
        setKpiTestError(`${first.stage}: ${first.message}`);
        setKpiValue(null);
        return;
      }
      const firstMapping = answer.mapping?.[0] as Record<string, unknown> | undefined;
      const currentValue = firstMapping?.currentValue as Record<string, unknown> | undefined;
      const isOk =
        answer.errors.length === 0 &&
        answer.validation.length > 0 &&
        answer.validation.every((v) => v.ok);
      // A value beside a closed Propose reads as a working indicator; say what is wrong
      // instead (UI-48, T-0911).
      setKpiValue(isOk ? (currentValue?.value ?? null) : null);
      if (!isOk) {
        const problem = answer.validation.flatMap((v) => v.problems ?? [])[0];
        setKpiTestError(
          answer.validation.length === 0
            ? t("pipelines.studio.kpi.nothingRead")
            : (problem ?? t("pipelines.test.gate")),
        );
      }
      onVerdict?.(isOk, bloblang);
    } catch {
      setKpiTestError(t("pipelines.test.failed", { status: 0 }));
    } finally {
      setKpiTesting(false);
    }
  }

  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  const spaceList = useMemo(() => asManifests(spaces.data?.items ?? []), [spaces.data]);
  const modelList = useMemo(() => asManifests(models.data?.items ?? []), [models.data]);

  const formKind = sourceKindOf(draft);
  const kind = formKind === "none" ? kindChoice : formKind;
  const endpointName = draft?.source?.endpointRef;
  const endpoint = endpoints.find((e) => e.metadata.name === endpointName);
  const space = (endpoint ? spaceOf(endpoint) : undefined) ?? spaceChoice;
  const spaceEndpoints = endpoints.filter((e) => spaceOf(e) === space);
  const spaceManifest = spaceList.find((s) => s.metadata.name === space);
  const model = modelList.find((m) => m.metadata.name === refName(spaceManifest?.spec.dataModelRef));
  const types = model ? entityTypesOf(model) : [];
  const type = draft?.source?.query?.type as string | undefined;
  const slots: FilterSlot[] = filterSlotsOf(useModelSource(project, model), type);
  if (slots.length === 0) {
    // No inline model: the sample's own keys are the attributes there are.
    for (const row of sample ?? []) {
      for (const key of Object.keys(row)) {
        if (!["id", "type", "@context"].includes(key) && !slots.some((slot) => slot.name === key)) {
          slots.push({ name: key, kind: "Property" });
        }
      }
    }
  }
  const attributes = slots.map((slot) => slot.name);
  const query: EntityQuery = {
    type,
    attrs: draft?.source?.query?.attrs as string[] | undefined,
    q: draft?.source?.query?.q as string | undefined,
    scopeQ: draft?.source?.query?.scopeQ as string | undefined,
  };
  const slug = typeof endpoint?.spec.slug === "string" ? (endpoint.spec.slug as string) : undefined;
  const access = useAccess(slug);
  const attributeAccess = useAttributeAccess(slug, type);
  const denied = deniedOf(attributeAccess.data, t);
  const ids = (draft?.source?.query?.ids as string[] | undefined) ?? [];

  function update(patch: (form: PipelineForm) => PipelineForm) {
    onChange(patch(draft ?? { class: "auto" }));
  }

  function setQuery(fields: Record<string, unknown>) {
    update((form) => ({
      ...form,
      source: { ...form.source, query: { ...form.source?.query, ...fields } },
    }));
  }

  function chooseKind(next: SourceKind) {
    setSample(null);
    setKindChoice(next);
    setSpaceChoice(undefined);
    update((form) => ({
      ...form,
      source: next === "none" ? undefined : {},
    }));
  }

  function chooseDataSource(name: string) {
    update((form) => ({
      ...form,
      source: name ? { dataSourceRef: name } : {},
    }));
  }

  function chooseSpace(name: string) {
    setSample(null);
    setSpaceChoice(name || undefined);
    const read = pickReadEndpoint(endpoints.filter((e) => spaceOf(e) === name));
    update((form) => ({
      ...form,
      // A page read once an hour unless the author already chose a cadence (PL-26).
      period: form.period ?? (form.schedule ? undefined : DEFAULT_PERIOD),
      source: read ? { endpointRef: read.metadata.name } : {},
    }));
  }

  function chooseEndpoint(name: string) {
    setSample(null);
    update((form) => ({
      ...form,
      source: { ...form.source, endpointRef: name, query: undefined },
    }));
  }

  function changeQuery(next: EntityQuery) {
    if (next.type !== type) {
      // Another type: the sample and the ticked ids belonged to the old one.
      setSample(null);
      setQuery({ ...next, ids: undefined });
      return;
    }
    setQuery({ ...next });
  }

  function toggleId(id: string, on: boolean) {
    const next = on ? [...ids, id] : ids.filter((i) => i !== id);
    setQuery({ ids: next.length > 0 ? next : undefined });
  }

  async function loadSample() {
    if (!slug || !type) {
      return;
    }
    setLoading(true);
    setSampleError(null);
    try {
      setSample((await fetchEntities(slug, query, { limit: SAMPLE_LIMIT })).rows);
    } catch (err) {
      setSample(null);
      setSampleError(err instanceof ApiError ? err.message : t("app.error.generic"));
    } finally {
      setLoading(false);
    }
  }

  function applyAggregate(aggregate: Aggregate) {
    if (!type || !space) {
      return;
    }
    const attribute = aggregateAttribute || attributes[0] || "value";
    const outputType = (draft?.output?.type as string | undefined) || `${type}Aggregate`;
    update((form) => ({
      ...form,
      compute: {
        kind: "bloblang",
        bloblang: aggregateBloblang(aggregate, {
          type,
          attribute,
          space,
          outputType,
        }),
      },
      output: { type: outputType, mode: form.output?.mode ?? "upsert" },
    }));
  }

  const sectionClass = "flex flex-col gap-3 rounded-md border border-border bg-surface-subtle p-3";

  return (
    <div className="flex flex-col gap-3" data-testid="pipeline-studio">
      <Field id="studio-preset" label={t("pipelines.studio.preset.title")} description={t("pipelines.studio.preset.hint")}>
        <Select
          id="studio-preset"
          value={preset}
          onChange={(event) => {
            const next = event.target.value as StudioPreset;
            setPreset(next);
            if (next === "kpi" && kpiEndpoint) {
              emitKpi({
                endpointName: kpiEndpoint,
                type: kpiType,
                attribute: kpiAttribute,
                aggregate: kpiAggregate,
                period: kpiPeriod,
                name: kpiName,
              });
            }
          }}
        >
          <option value="load">{t("pipelines.studio.preset.load")}</option>
          <option value="kpi">{t("pipelines.studio.preset.kpi")}</option>
        </Select>
      </Field>

      {preset === "kpi" ? (
        <section
          className={sectionClass}
          aria-labelledby="studio-kpi-title"
          data-testid="studio-kpi-panel"
        >
          <FormHeading id="studio-kpi-title" className="text-body font-semibold text-fg">
            {t("pipelines.studio.kpi.title")}
          </FormHeading>
          <p className="text-caption text-fg-muted">{t("pipelines.studio.kpi.lead")}</p>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field id="studio-kpi-endpoint" label={t("pipelines.studio.kpi.endpoint")} description={t("pipelines.studio.kpi.endpointHint")}>
              <Select
                id="studio-kpi-endpoint"
                value={kpiEndpoint}
                onChange={(event) => {
                  const ep = event.target.value;
                  setKpiEndpoint(ep);
                  emitKpi({
                    endpointName: ep,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              >
                <option value="">—</option>
                {endpoints.map((e) => (
                  <option key={e.metadata.name} value={e.metadata.name}>
                    {localized(e.metadata.title, locale, e.metadata.name)}
                  </option>
                ))}
              </Select>
            </Field>
            <Field id="studio-kpi-name" label={t("pipelines.studio.kpi.name")} description={t("pipelines.studio.kpi.nameHint")}>
              <Input
                id="studio-kpi-name"
                value={kpiName}
                onChange={(event) => {
                  const name = event.target.value;
                  setKpiName(name);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name,
                  });
                }}
              />
            </Field>
            <Field id="studio-kpi-type" label={t("pipelines.studio.kpi.type")} description={t("pipelines.studio.kpi.typeHint")}>
              {/* The type is one the endpoint's space models (ADR-N-033), never typed. */}
              <TypePicker
                id="studio-kpi-type"
                label={t("pipelines.studio.kpi.type")}
                labelled
                project={project}
                space={(() => {
                  const ep = endpoints.find((e) => e.metadata.name === kpiEndpoint);
                  return ep ? spaceOf(ep) : undefined;
                })()}
                value={kpiType ? [kpiType] : []}
                onChange={(types) => {
                  const nextType = types[0] ?? "";
                  setKpiType(nextType);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: nextType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              />
            </Field>
            <Field
              id="studio-kpi-attribute"
              label={t("pipelines.studio.kpi.attribute")}
              description={t("pipelines.studio.kpi.attributePick")}
            >
              <AttributePicker
                id="studio-kpi-attribute"
                label={t("pipelines.studio.kpi.attribute")}
                labelled
                project={project}
                space={(() => {
                  const ep = endpoints.find((e) => e.metadata.name === kpiEndpoint);
                  return ep ? spaceOf(ep) : undefined;
                })()}
                type={kpiType || undefined}
                kind="Property"
                numeric
                value={kpiAttribute ? [kpiAttribute] : []}
                onChange={(names) => {
                  const nextAttr = names[0] ?? "";
                  setKpiAttribute(nextAttr);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: nextAttr,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              />
            </Field>
            <Field id="studio-kpi-agg" label={t("pipelines.studio.kpi.aggregate")} description={t("pipelines.studio.kpi.aggregateHint")}>
              <Select
                id="studio-kpi-agg"
                value={kpiAggregate}
                onChange={(event) => {
                  const nextAgg = event.target.value as "average" | "sum" | "count";
                  setKpiAggregate(nextAgg);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: nextAgg,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              >
                <option value="average">{t("pipelines.studio.aggregate.average")}</option>
                <option value="sum">{t("pipelines.studio.aggregate.sum")}</option>
                <option value="count">{t("pipelines.studio.aggregate.count")}</option>
              </Select>
            </Field>
            <Field id="studio-kpi-period" label={t("pipelines.studio.kpi.period")} description={t("pipelines.studio.kpi.periodHint")}>
              <Input
                id="studio-kpi-period"
                value={kpiPeriod}
                onChange={(event) => {
                  const nextPeriod = event.target.value;
                  setKpiPeriod(nextPeriod);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: nextPeriod,
                    name: kpiName,
                  });
                }}
              />
            </Field>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <Button
              size="sm"
              variant="primary"
              data-testid="studio-kpi-test"
              disabled={!kpiEndpoint || kpiTesting}
              disabledReason={kpiTesting ? t("app.busyReason") : t("pipelines.studio.kpi.pickEndpoint")}
              onClick={() => {
                void runKpiTest();
              }}
            >
              {kpiTesting ? t("pipelines.studio.kpi.testing") : t("pipelines.studio.kpi.test")}
            </Button>
            {kpiValue !== null && kpiValue !== undefined ? (
              <span className="text-caption text-fg">
                {t("pipelines.studio.kpi.computedValue")}:{" "}
                <strong data-testid="studio-kpi-value">{String(kpiValue)}</strong>
              </span>
            ) : null}
          </div>

          {kpiTestError ? (
            <Alert role="alert" tone="danger">
              {kpiTestError}
            </Alert>
          ) : null}
        </section>
      ) : (
        <>
          <section className={sectionClass} aria-labelledby="studio-flow">
            <div className="flex items-center justify-between">
              <FormHeading id="studio-flow" className="text-body font-semibold text-fg">
                {t("pipelines.flow.canvas", { defaultValue: "Pipeline canvas" })}
              </FormHeading>
              <div
                role="tablist"
                aria-label={t("pipelines.flow.canvas", { defaultValue: "Pipeline canvas" })}
                className="flex items-center gap-1"
              >
                <Button
                  role="tab"
                  size="sm"
                  variant={studioView === "flow" ? "secondary" : "ghost"}
                  aria-selected={studioView === "flow"}
                  data-testid="studio-view-flow"
                  onClick={() => setStudioView("flow")}
                >
                  {t("pipelines.flow.view.flow", { defaultValue: "Flow" })}
                </Button>
                <Button
                  role="tab"
                  size="sm"
                  variant={studioView === "form" ? "secondary" : "ghost"}
                  aria-selected={studioView === "form"}
                  data-testid="studio-view-form"
                  onClick={() => setStudioView("form")}
                >
                  {t("pipelines.flow.view.form", { defaultValue: "Form" })}
                </Button>
              </div>
            </div>

            {studioView === "flow" ? (
              <div className="flex flex-col gap-3">
                <PipelineFlow
                  form={draft}
                  onChange={onChange}
                  trace={flowTrace}
                  live={liveCounters}
                  bufferDepth={bufferQuery.data?.bufferDepth}
                  selected={selectedNode}
                  onSelect={setSelectedNode}
                  onOpen={(id) => {
                    setSelectedNode(id);
                    setOpening((was) => ({ id, count: (was?.count ?? 0) + 1 }));
                  }}
                  taps={taps}
                  onTap={(key) => setTaps((was) => (was.includes(key) ? was.filter((one) => one !== key) : [...was, key]))}
                  dataSources={dataSources}
                  endpoints={endpoints}
                />
                {statsUnavailable && !flowTrace ? (
                  <p className="text-caption text-fg-muted" data-testid="flow-live-unavailable">
                    {t("pipelines.flow.liveUnavailable")}
                  </p>
                ) : null}
                {lastErrorOfNode ? (
                  <p className="text-caption text-danger-fg" data-testid="flow-last-error">
                    {t("pipelines.flow.lastError", { at: lastErrorOfNode.at, message: lastErrorOfNode.message })}
                  </p>
                ) : null}

                {selectedNode === "compute" ? (
                  <div
                    className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3"
                    data-testid="flow-node-editor-compute"
                  >
                    <div className="flex items-center justify-between">
                      <span className="text-caption font-semibold text-fg">
                        {t("pipelines.flow.node.compute", { defaultValue: "Compute" })} (
                        {draft?.compute?.kind ??
                          t("pipelines.flow.passThrough", { defaultValue: "pass-through" })}
                        )
                      </span>
                      {draft?.compute?.kind ? (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            onChange(setComputeKind(draft, null));
                          }}
                        >
                          {t("pipelines.flow.remove", { defaultValue: "Remove" })}
                        </Button>
                      ) : null}
                    </div>
                    {draft?.compute?.kind && draft.compute.kind !== "bloblang" ? (
                      // Only a Bloblang step runs today (Architecture/08): say so where the kind
                      // is chosen, not after the merge as a Pending nobody can explain (T-3088).
                      <Alert tone="warning" data-testid="flow-compute-not-run">
                        {t("pipelines.flow.computeNotRun", {
                          kind: t(`pipelines.field.computeChoice.${draft.compute.kind}`),
                        })}
                      </Alert>
                    ) : null}
                    {draft?.compute?.kind === "bloblang" ? (
                      <Field id="flow-bloblang-field" label={t("pipelines.flow.nodeBloblang")} description={t("pipelines.field.bloblangHint")}>
                        <Textarea
                          id="flow-bloblang-field"
                          data-testid="flow-bloblang"
                          rows={6}
                          className="font-mono text-caption"
                          value={draft?.compute?.bloblang ?? ""}
                          onChange={(e) => {
                            onChange({
                              ...draft,
                              compute: {
                                ...draft?.compute,
                                kind: "bloblang",
                                bloblang: e.target.value,
                              },
                            });
                          }}
                        />
                      </Field>
                    ) : draft?.compute?.kind === "mapping" ? (
                      <Field id="flow-mapping-field" label={t("pipelines.field.mappingRef")} description={t("pipelines.flow.mappingRefHint")}>
                        <ResourceNamePicker
                          id="flow-mapping-field"
                          label={t("pipelines.field.mappingRef")}
                          labelled
                          from={{ project, plural: "mappings" }}
                          value={draft?.compute?.mappingRef ?? ""}
                          onChange={(mappingRef) => {
                            onChange({
                              ...draft,
                              compute: {
                                ...draft?.compute,
                                kind: "mapping",
                                mappingRef,
                              },
                            });
                          }}
                        />
                      </Field>
                    ) : draft?.compute?.kind === "wasm" || draft?.compute?.kind === "container" ? (
                      <div className="grid gap-2 sm:grid-cols-2">
                        <Field id="flow-module-field" label={t("pipelines.field.module")} description={t("pipelines.flow.moduleHint")}>
                          <Input
                            id="flow-module-field"
                            data-testid="flow-module"
                            value={draft?.compute?.module ?? ""}
                            onChange={(e) => {
                              onChange({
                                ...draft,
                                compute: {
                                  ...draft?.compute,
                                  module: e.target.value,
                                },
                              });
                            }}
                          />
                        </Field>
                        <Field id="flow-function-field" label={t("pipelines.field.function")} description={t("pipelines.flow.functionHint")}>
                          <Input
                            id="flow-function-field"
                            data-testid="flow-function"
                            value={draft?.compute?.function ?? ""}
                            onChange={(e) => {
                              onChange({
                                ...draft,
                                compute: {
                                  ...draft?.compute,
                                  function: e.target.value,
                                },
                              });
                            }}
                          />
                        </Field>
                      </div>
                    ) : (
                      <p className="text-caption text-fg-muted">
                        {t("pipelines.flow.passThrough", {
                          defaultValue: "Pass-through (no compute step)",
                        })}
                      </p>
                    )}
                  </div>
                ) : selectedSource ? (
                  <SourceBlock
                    project={project}
                    source={selectedSource}
                    dataSources={dataSources}
                    endpoints={endpoints}
                    locale={locale}
                    onChange={(source) =>
                      onChange({
                        ...draft,
                        moreSources: (draft?.moreSources ?? []).map((was, at) =>
                          at === sourceIndexOf(selectedNode) ? source : was,
                        ),
                      })
                    }
                    onRemove={() => {
                      if (draft) onChange(removeSource(draft, sourceIndexOf(selectedNode) ?? -1));
                      setSelectedNode(null);
                    }}
                  />
                ) : selectedStep ? (
                  <StepBlock
                    variables={(draft?.secretRefs ?? []).flatMap((ref) => (ref.envVar ? [ref.envVar] : []))}
                    key={selectedNode}
                    entry={selectedStep}
                    onChange={(entry) =>
                      onChange({
                        ...draft,
                        processors: (draft?.processors ?? []).map((was, at) =>
                          at === stepIndexOf(selectedNode) ? entry : was,
                        ),
                      })
                    }
                    onRemove={() => {
                      if (draft) onChange(removeStep(draft, stepIndexOf(selectedNode) ?? -1));
                      setSelectedNode(null);
                    }}
                  />
                ) : outputIndexOf(selectedNode) !== undefined && draft?.moreOutputs?.[outputIndexOf(selectedNode) ?? -1] ? (
                  <OutputBlock
                    // A second output is edited here, in place (T-3088); the first is the form's.
                    key={selectedNode}
                    project={project}
                    output={draft.moreOutputs[outputIndexOf(selectedNode) ?? -1]}
                    targets={endpoints.map((manifest) => ({ manifest, urn: endpointUrn(orgDomain, manifest) }))}
                    locale={locale}
                    onChange={(output) =>
                      onChange({
                        ...draft,
                        moreOutputs: (draft.moreOutputs ?? []).map((was, at) =>
                          at === outputIndexOf(selectedNode) ? output : was,
                        ),
                      })
                    }
                    onRemove={() => {
                      onChange(removeOutput(draft, outputIndexOf(selectedNode) ?? -1));
                      setSelectedNode(null);
                    }}
                  />
                ) : routeNodeOf(selectedNode) && draft?.routes?.[routeNodeOf(selectedNode)?.route ?? -1] ? (
                  (() => {
                    const inRoute = routeNodeOf(selectedNode) as { route: number; step?: number; output?: number };
                    const route = draft.routes?.[inRoute.route] ?? {};
                    const setRoute = (changed: RouteForm) =>
                      onChange({ ...draft, routes: (draft.routes ?? []).map((was, at) => (at === inRoute.route ? changed : was)) });
                    const step = route.steps?.[inRoute.step ?? -1];
                    return step ? (
                      <StepBlock
                        key={selectedNode}
                        entry={{ step }}
                        onChange={(entry) =>
                          setRoute({ ...route, steps: (route.steps ?? []).map((was, at) => (at === inRoute.step ? entry.step : was)) })
                        }
                        onRemove={() => {
                          onChange(removeRouteNode(draft, selectedNode as FlowNodeId));
                          setSelectedNode(null);
                        }}
                      />
                    ) : (
                      <RouteBlock
                        key={selectedNode}
                        project={project}
                        route={route}
                        output={inRoute.output}
                        targets={endpoints.map((manifest) => ({ manifest, urn: endpointUrn(orgDomain, manifest) }))}
                        locale={locale}
                        onChange={setRoute}
                        onRemove={() => {
                          onChange(removeRoute(draft, inRoute.route));
                          setSelectedNode(null);
                        }}
                      />
                    );
                  })()
                ) : selectedNode === "source" ? (
                  // The first source opens the same block as the others (T-3978); it stays.
                  <SourceBlock
                    key="source"
                    project={project}
                    source={draft?.source ?? {}}
                    dataSources={dataSources}
                    endpoints={endpoints}
                    locale={locale}
                    onChange={(source) => {
                      onChange({ ...draft, source });
                      setKindChoice(sourceKindOf({ source }));
                    }}
                  />
                ) : selectedNode === "output" ? (
                  <OutputBlock
                    key="output"
                    project={project}
                    output={{ targetEndpoint: draft?.targetEndpoint, ...draft?.output }}
                    targets={endpoints.map((manifest) => ({ manifest, urn: endpointUrn(orgDomain, manifest) }))}
                    locale={locale}
                    onChange={({ targetEndpoint, ...output }) => onChange({ ...draft, targetEndpoint, output })}
                  />
                ) : null}

                {!flowTrace && selectedNode && pipelineName ? (
                  <LastSamples project={project} pipeline={pipelineName} node={selectedNode} />
                ) : null}
                {flowTrace && selectedNode ? (
                  <div className="grid gap-2 sm:grid-cols-2" data-testid="flow-sample-drawer">
                    <div className="flex flex-col gap-1 rounded-md border border-border bg-surface p-2">
                      <span className="text-caption font-semibold text-fg">
                        {t("pipelines.flow.samplesIn", { defaultValue: "Input sample" })}
                      </span>
                      {(() => {
                        const sampleIn = samplesOf(flowTrace, toFlow(draft).nodes, selectedNode).in;
                        return sampleIn !== undefined ? (
                          <RecordView
                            value={sampleIn}
                            label={t("pipelines.flow.samplesIn", { defaultValue: "Input sample" })}
                            data-testid="flow-sample-in"
                            className="max-h-36"
                          />
                        ) : (
                          <p data-testid="flow-sample-in" className="text-caption text-fg-muted">
                            {t("pipelines.flow.noSample", { defaultValue: "No sample" })}
                          </p>
                        );
                      })()}
                    </div>
                    <div className="flex flex-col gap-1 rounded-md border border-border bg-surface p-2">
                      <span className="text-caption font-semibold text-fg">
                        {t("pipelines.flow.samplesOut", { defaultValue: "Output sample" })}
                      </span>
                      {(() => {
                        const sampleOut = samplesOf(flowTrace, toFlow(draft).nodes, selectedNode).out;
                        return sampleOut !== undefined ? (
                          <RecordView
                            value={sampleOut}
                            label={t("pipelines.flow.samplesOut", { defaultValue: "Output sample" })}
                            data-testid="flow-sample-out"
                            className="max-h-36"
                          />
                        ) : (
                          <p data-testid="flow-sample-out" className="text-caption text-fg-muted">
                            {t("pipelines.flow.noSample", { defaultValue: "No sample" })}
                          </p>
                        );
                      })()}
                    </div>
                  </div>
                ) : null}
                {flowTrace && selectedNode && draft && toManifest ? (
                  <NodeDebug
                    key={selectedNode}
                    project={project}
                    form={draft}
                    trace={flowTrace}
                    nodes={toFlow(draft).nodes}
                    id={selectedNode}
                    toManifest={toManifest}
                  />
                ) : null}
                <DebugSidebar
                  log={debugLog}
                  nodes={toFlow(draft).nodes}
                  edges={toFlow(draft).edges}
                  taps={taps}
                  nameOf={(id) =>
                    `${t(`pipelines.flow.node.${id.startsWith("step-") ? "step" : id.startsWith("source") ? "source" : id.startsWith("output") ? "output" : "compute"}`)} ${toFlow(draft).nodes.find((node) => node.id === id)?.kind ?? ""}`.trim()
                  }
                  onClear={() => setDebugLog([])}
                />
              </div>
            ) : null}
          </section>

          {toManifest ? (
            <PipelineTest
              project={project}
              draft={draft}
              onChange={onChange}
              toManifest={toManifest}
              sampleUrl={sampleUrlOf(draft, dataSources, endpoints)}
              sourceBody={sourceBodyOf(draft, dataSources)}
              onVerdict={onVerdict}
              onTrace={setFlowTrace}
              onDebug={(entry) => setDebugLog((was) => [...was, entry].slice(-100))}
            />
          ) : null}
          <section className={sectionClass} aria-labelledby="studio-source">
            <FormHeading id="studio-source" className="text-body font-semibold text-fg">
              {t("pipelines.studio.source")}
            </FormHeading>
            <Field id="studio-source-kind" label={t("pipelines.studio.sourceKind")} description={t("pipelines.studio.sourceKindHint")}>
              <Select
                id="studio-source-kind"
                value={kind}
                onChange={(event) => chooseKind(event.target.value as SourceKind)}
              >
                <option value="none">{t("pipelines.studio.kind.none")}</option>
                <option value="datasource">{t("pipelines.studio.kind.datasource")}</option>
                <option value="space">{t("pipelines.studio.kind.space")}</option>
              </Select>
            </Field>
            {kind === "datasource" ? (
              <Field id="studio-datasource" label={t("pipelines.field.dataSource")} description={t("pipelines.studio.dataSourceHint")}>
                <Select
                  id="studio-datasource"
                  value={draft?.source?.dataSourceRef ?? ""}
                  onChange={(event) => chooseDataSource(event.target.value)}
                >
                  <option value="">—</option>
                  {dataSources.map((source) => (
                    <option key={source.metadata.name} value={source.metadata.name}>
                      {localized(source.metadata.title, locale, source.metadata.name)}
                    </option>
                  ))}
                </Select>
              </Field>
            ) : null}
            {kind === "space" ? (
              <div className="grid gap-3 sm:grid-cols-2">
                <Field id="studio-space" label={t("pipelines.studio.space")} description={t("pipelines.studio.spaceHint")}>
                  <Select
                    id="studio-space"
                    value={space ?? ""}
                    onChange={(event) => chooseSpace(event.target.value)}
                  >
                    <option value="">—</option>
                    {spaceList.map((s) => (
                      <option key={s.metadata.name} value={s.metadata.name}>
                        {localized(s.metadata.title, locale, s.metadata.name)}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field
                  id="studio-endpoint"
                  label={t("pipelines.studio.readThrough")}
                  description={t("pipelines.studio.readThroughHint")}
                >
                  <Select
                    id="studio-endpoint"
                    value={endpointName ?? ""}
                    disabled={!space}
                    title={!space ? t("pipelines.studio.pickSpaceFirst") : undefined}
                    onChange={(event) => chooseEndpoint(event.target.value)}
                  >
                    <option value="">—</option>
                    {spaceEndpoints.map((e) => (
                      <option key={e.metadata.name} value={e.metadata.name}>
                        {localized(e.metadata.title, locale, e.metadata.name)}
                      </option>
                    ))}
                  </Select>
                </Field>
              </div>
            ) : null}
          </section>

          {kind === "space" && space ? (
            <section className={sectionClass} aria-labelledby="studio-entities">
              <FormHeading id="studio-entities" className="text-body font-semibold text-fg">
                {t("pipelines.studio.entities")}
              </FormHeading>
              <EntityFilters
                id="studio"
                types={types}
                space={space}
                slots={slots}
                value={query}
                onChange={changeQuery}
                denied={denied}
              />
              <AccessPanel slug={slug} type={type} access={access} />
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  size="sm"
                  disabled={!slug || !type || loading}
                  disabledReason={loading ? t("app.busyReason") : t("pipelines.studio.pickTypeFirst")}
                  onClick={() => {
                    void loadSample();
                  }}
                >
                  {loading ? t("app.loading") : t("pipelines.studio.loadSample")}
                </Button>
                <span className="text-caption text-fg-muted">
                  {slug
                    ? t("pipelines.studio.sampleHint", { count: SAMPLE_LIMIT })
                    : t("pipelines.studio.noEndpoint")}
                </span>
              </div>
              {sampleError ? (
                <Alert role="alert" tone="danger">
                  {sampleError}
                </Alert>
              ) : null}
              {sample ? (
                sample.length === 0 ? (
                  <p className="text-caption text-fg-muted">{t("pipelines.studio.noEntities")}</p>
                ) : (
                  // `maxHeight`: the sample is up to SAMPLE_LIMIT entities, and a long one
                  // scrolls in its own frame rather than pushing the rest of the studio down.
                  <Table
                    caption={t("pipelines.studio.sample")}
                    zebra={false}
                    maxHeight="max-h-64"
                    className="text-caption"
                  >
                    <TableHead>
                      <TableHeaderCell>
                        <span className="sr-only">{t("pipelines.studio.tick")}</span>
                      </TableHeaderCell>
                      <TableHeaderCell>id</TableHeaderCell>
                      {attributes.slice(0, 4).map((attribute) => (
                        <TableHeaderCell key={attribute} className="font-mono normal-case">
                          {attribute}
                        </TableHeaderCell>
                      ))}
                    </TableHead>
                    <TableBody>
                      {sample.map((row) => (
                        <TableRow key={row.id}>
                          <TableCell>
                            <Checkbox
                              label={<span className="sr-only">{row.id}</span>}
                              checked={ids.includes(row.id)}
                              onChange={(event) => toggleId(row.id, event.target.checked)}
                            />
                          </TableCell>
                          <TableCell primary className="font-mono">
                            {row.id}
                          </TableCell>
                          {attributes.slice(0, 4).map((attribute) => (
                            <TableCell key={attribute} className="font-mono">
                              {attribute in row ? cell(row[attribute]) : ""}
                            </TableCell>
                          ))}
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                )
              ) : null}
              {ids.length > 0 ? (
                <p className="text-caption text-fg-muted">
                  {t("pipelines.studio.ticked", { count: ids.length })}
                </p>
              ) : null}
            </section>
          ) : null}

          {kind === "space" && type ? (
            <section className={sectionClass} aria-labelledby="studio-process">
              <FormHeading id="studio-process" className="text-body font-semibold text-fg">
                {t("pipelines.studio.process")}
              </FormHeading>
              <p className="text-caption text-fg-muted">{t("pipelines.studio.processHint")}</p>
              <div className="flex flex-wrap items-end gap-2">
                <Field id="studio-aggregate-attribute" label={t("pipelines.studio.attribute")} description={t("pipelines.studio.attributeHint")}>
                  {/* The model's attributes of the type, else the sample's own keys: never typed. */}
                  <Combobox
                    id="studio-aggregate-attribute"
                    label={t("pipelines.studio.attribute")}
                    labelled
                    options={attributes.map((name) => ({ value: name, label: name, group: type ?? "" }))}
                    empty={t("pipelines.studio.noAttributes")}
                    value={aggregateAttribute || attributes[0] ? [aggregateAttribute || attributes[0]] : []}
                    onChange={(names) => setAggregateAttribute(names[0] ?? "")}
                  />
                </Field>
                {AGGREGATES.map((aggregate) => (
                  <Button key={aggregate} size="sm" onClick={() => applyAggregate(aggregate)}>
                    {t(`pipelines.studio.aggregate.${aggregate}`)}
                  </Button>
                ))}
              </div>
            </section>
          ) : null}
        </>
      )}
    </div>
  );
}
