import { RecipeGallery } from "./RecipeGallery";
import { SaveAsTemplate, TemplateList } from "./PipelineTemplates";
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { ListFailed } from "../../components/forms/widgets/ListFailed";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, reasonOf } from "../../api/client";
import { asManifests, knownSecretNames, overlay, plainTitle, prune, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { allowsIn, usePermissions } from "../../api/permissions";
import type { Effective } from "../../api/permissions";
import { useOrgDomain } from "../../api/projects";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import type { ManifestSource } from "../../components/ResourceFormDialog";
import { Alert, buttonClass, Icon, safeHref } from "../../components/ui";
import { pipelineSchema, pipelineUiSchemaFor } from "../../schemas/kinds";
import type { EndpointOption, PipelineShown } from "../../schemas/kinds";
import { PipelineStudio } from "./PipelineStudio";
import { PipelineWorkbench } from "./PipelineWorkbench";
import type { MappingFile } from "./PipelineWorkbench";
import type { UiSchema } from "../../components/forms/types";
import type { History } from "./history";
import type { Lifetime } from "./lifetime";

/** The form of a pipeline: `PipelineSpec` with every reference flattened to its name. */
export interface PipelineForm {
  name?: string;
  title?: string;
  class?: string;
  schedule?: string;
  period?: string;
  source?: SourceForm;
  /**
   * The sources after the first, in order (PL-52). The first one stays `source`: it is the one
   * the sample, the KPI preset and the access panel read. The runner merges them all through a
   * `broker` input (PL-53).
   */
  moreSources?: SourceForm[];
  compute?: {
    kind?: string;
    module?: string;
    function?: string;
    mappingRef?: string;
    bloblang?: string;
  };
  /**
   * The runner processors around the compute step, in order (PL-52). `step` is the manifest's own
   * step, kept whole so an edit loses nothing of it; `after` puts it behind the compute step.
   */
  processors?: StepForm[];
  targetEndpoint?: string;
  output?: { type?: string; mode?: string };
  /**
   * The outputs after the first, in order (PL-52, PL-55): each writes through its own Endpoint.
   * The runner fans the messages out to all of them, every write one must succeed.
   */
  moreOutputs?: OutputForm[];
  /**
   * Routes by condition (PL-73): after the lane, a record takes the first route whose `when`
   * holds, runs its steps and is written through its outputs; the rest go to the outputs above.
   */
  routes?: RouteForm[];
  allowFeedback?: boolean;
  /** Stale-entity expiry: off unless both are given (PL-64). */
  expiry?: { after?: string; types?: string[] };
  /** When entities and attributes end by themselves, the runner's step after the mapping (PL-80). */
  lifetime?: Lifetime[];
  /** Which history rows the broker keeps of each written attribute, and the time of a row (PL-106). */
  history?: History[];
  secretRefs?: { name?: string; key?: string; envVar?: string }[];
  quotas?: { maxMemoryMb?: number; cpuMillicores?: number };
}

/** One output after the first: the Endpoint's URN, the type written and the write mode (PL-55). */
export interface OutputForm {
  targetEndpoint?: string;
  type?: string;
  mode?: string;
}

/** One route (PL-73), as the manifest holds it: its steps are the manifest's own, kept whole. */
export interface RouteForm {
  name?: string;
  when?: string;
  steps?: Record<string, unknown>[];
  outputs?: OutputForm[];
}

/** One source a pipeline reads: a DataSource, or an Endpoint with a query or a trigger (PL-52). */
export interface SourceForm {
  dataSourceRef?: string;
  endpointRef?: string;
  query?: Record<string, unknown>;
  trigger?: {
    subscription?: { type?: string; watchedAttributes?: string[] };
  };
}

/** One step of the lane that is not the form's compute step (PL-52). */
export interface StepForm {
  step: Record<string, unknown>;
  after?: boolean;
}

const PLURAL = "pipelines";

function typedRef(kind: string, name: string | undefined) {
  return name ? { kind, name } : undefined;
}

/** One source as the manifest holds it: its references typed (MF-07). */
function writeSource(source: SourceForm): Record<string, unknown> {
  return {
    ...source,
    dataSourceRef: typedRef("DataSource", source.dataSourceRef),
    endpointRef: typedRef("Endpoint", source.endpointRef),
  };
}

/** One source as the form holds it: its references by name, whichever way they were written. */
function readSource(source: Record<string, unknown>): SourceForm {
  return {
    ...source,
    dataSourceRef: refName(source.dataSourceRef) || undefined,
    endpointRef: refName(source.endpointRef) || undefined,
  };
}

/** The sources after the first, written; an empty list stays a list, so a removal is kept. */
function sourcesFor(moreSources: SourceForm[] | undefined): Record<string, unknown>[] | undefined {
  return moreSources?.map(writeSource);
}

/**
 * The URN a pipeline writes through: `urn:ngsi-ld:Endpoint:{orgDomain}:{space}:{name}`
 * (PF-39, PL-18). The organization's domain comes from the branding; without it, or without
 * the endpoint's space, there is no URN to offer and the field takes one typed in full.
 */
export function endpointUrn(orgDomain: string, endpoint: Manifest): string | undefined {
  const space = (endpoint.spec as { contextSpaceRef?: unknown }).contextSpaceRef;
  if (!orgDomain || typeof space !== "string" || !space) {
    return undefined;
  }
  return `urn:ngsi-ld:Endpoint:${orgDomain}:${space}:${endpoint.metadata.name}`;
}

/**
 * The Endpoints a person may name in a pipeline (PL-94): a public one, or one into a space where
 * they read `Entity`. The server refuses the rest at every door with "No access to …"; a target
 * also needs `propose` on Pipeline there, which the API judges.
 */
export function reachableEndpoints(effective: Effective | undefined, endpoints: Manifest[]): Manifest[] {
  return endpoints.filter((endpoint) => {
    const spec = endpoint.spec as { audience?: unknown; contextSpaceRef?: unknown };
    const space = refName(spec.contextSpaceRef);
    return spec.audience === "public" || (space !== "" && allowsIn(effective, "Entity", "read", space));
  });
}

/** The spec keys the form writes; the rest of an edited manifest travels as it is (T-0885). */
const OWNED_SPEC = [
  "class",
  "schedule",
  "period",
  "source",
  "compute",
  "targetEndpoint",
  "output",
  "allowFeedback",
  "expiry",
  "lifetime",
  "history",
  "secretRefs",
  "quotas",
  "sources",
  "steps",
  "outputs",
  "routes",
];

/** The version a Pipeline is written at whenever it has a source (PL-54, ADR-N-023). */
const SECOND_VERSION = "joinedcontext.com/v1alpha2";

type Spec = Record<string, unknown>;

const listOf = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
const isComputeStep = (step: unknown): boolean =>
  typeof step === "object" && step !== null && "kind" in step && !("processor" in step);

/**
 * The form's one source, compute and output written as `sources`, `steps` and `outputs`
 * (PL-54). The form shows the first of each; what else the edited manifest holds (a second
 * source, a processor step, a second output) is kept where it was.
 */
function secondShape(
  spec: Spec,
  base: Spec,
  processors?: StepForm[],
  moreSources?: SourceForm[],
  moreOutputs?: OutputForm[],
  routes?: RouteForm[],
): Spec {
  const { source, compute, targetEndpoint, output, ...others } = spec;
  const baseSteps = listOf(base.steps);
  const at = baseSteps.findIndex(isComputeStep);
  // A form that carries the lane owns its order; one that does not (a form filled by hand in a
  // test, an older caller) keeps whatever steps the edited manifest held.
  const steps = processors
    ? [
        ...processors.filter((entry) => !entry.after).map((entry) => entry.step),
        ...(compute === undefined ? [] : [compute]),
        ...processors.filter((entry) => entry.after).map((entry) => entry.step),
      ]
    : compute === undefined
      ? baseSteps.filter((_, index) => index !== at)
      : at >= 0
        ? baseSteps.map((step, index) => (index === at ? compute : step))
        : [compute, ...baseSteps];
  return prune({
    ...others,
    // Same rule as the steps: a form that carries the rest of the sources owns them; one that
    // does not keeps whatever the edited manifest held.
    sources: [source, ...(moreSources ?? (listOf(base.sources).slice(1) as SourceForm[]))],
    steps: steps.length > 0 ? steps : undefined,
    outputs: [
      { targetEndpoint, ...((output as Spec | undefined) ?? {}) },
      ...(moreOutputs ? moreOutputs.map((more) => prune({ ...more }) as Spec) : listOf(base.outputs).slice(1)),
    ],
    // A form that carries routes owns them, an emptied list included; one that does not keeps
    // the edited manifest's.
    routes: (routes ?? (listOf(base.routes) as RouteForm[])).map((route) => prune({ ...route }) as Spec),
  }) as Spec;
}

/** The sources after the first, as the form carries them (PL-52). */
export function moreSourcesOf(spec: Spec): SourceForm[] | undefined {
  const rest = listOf(spec.sources).slice(1) as Record<string, unknown>[];
  return rest.length > 0 ? rest.map(readSource) : undefined;
}

/** The outputs after the first, as the form carries them (PL-55). */
export function moreOutputsOf(spec: Spec): OutputForm[] | undefined {
  const rest = listOf(spec.outputs).slice(1) as OutputForm[];
  return rest.length > 0 ? rest.map((output) => ({ ...output })) : undefined;
}

/** The steps around the first compute step, as the form carries them (PL-52). */
export function processorsOf(spec: Spec): StepForm[] | undefined {
  const steps = listOf(spec.steps) as Record<string, unknown>[];
  const at = steps.findIndex(isComputeStep);
  const around = steps
    .map((step, index) => ({ step, index }))
    .filter(({ index }) => index !== at)
    .map(({ step, index }) => (at >= 0 && index > at ? { step, after: true } : { step }));
  return around.length > 0 ? around : undefined;
}

/**
 * The spec in the first shape the form edits (PL-54): `v1alpha2`'s first source, first compute
 * step and first output, so the form reads a manifest written in either version.
 */
export function firstShape(spec: Spec): Spec {
  if (!("sources" in spec || "steps" in spec || "outputs" in spec)) {
    return spec;
  }
  const { sources, steps, outputs, ...others } = spec;
  const [output] = listOf(outputs) as Spec[];
  const { targetEndpoint, ...written } = output ?? {};
  return prune({
    ...others,
    source: listOf(sources)[0],
    compute: listOf(steps).find(isComputeStep),
    targetEndpoint,
    output: Object.keys(written).length > 0 ? written : undefined,
  }) as Spec;
}

/**
 * The manifest a form produces (PL-04, PL-31, PL-33, PL-39). `base` is the manifest being
 * edited, from the list or from the YAML view: what the form does not show (`enabled`, set by
 * the pause button, or any field it has no control for) survives an edit.
 */
export function toEnvelope(project: string, form: PipelineForm, base?: Manifest): Manifest {
  const {
    name,
    title,
    source,
    moreSources,
    moreOutputs,
    routes,
    compute,
    allowFeedback,
    expiry,
    lifetime,
    history,
    secretRefs,
    quotas,
    processors,
    ...rest
  } = form;
  const spec = prune({
    class: rest.class,
    schedule: rest.schedule,
    period: rest.period,
    source: source ? writeSource(source) : undefined,
    compute: compute
      ? { ...compute, mappingRef: typedRef("Mapping", compute.mappingRef) }
      : undefined,
    targetEndpoint: rest.targetEndpoint,
    output: rest.output,
    allowFeedback: allowFeedback ? true : undefined,
    // An emptied expiry is gone, not an empty object the manifest refuses: off means absent.
    expiry: expiry?.after || expiry?.types?.length ? expiry : undefined,
    lifetime: lifetime?.length ? lifetime : undefined,
    history: history?.length ? history : undefined,
    secretRefs,
    quotas,
  }) as Record<string, unknown>;
  // A pipeline whose input lives in bento.yaml has no source and stays at v1alpha1.
  const second = Boolean(spec.source && spec.targetEndpoint);
  const next: Manifest = {
    apiVersion: second ? SECOND_VERSION : "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: {
      name: name ?? "",
      namespace: project,
      ...(title?.trim() ? { title } : {}),
    },
    spec: second
      ? secondShape(spec, (base?.spec ?? {}) as Spec, processors, sourcesFor(moreSources), moreOutputs, routes)
      : spec,
  };
  return { ...overlay(base, next, OWNED_SPEC), apiVersion: next.apiVersion };
}

/**
 * What the canvas holds that the manifest cannot carry, as the key of the sentence saying so and
 * the field that fixes it (T-3969). Extra sources, processor steps and extra outputs exist only
 * in `v1alpha2`, which needs a first source and a target: without them `toEnvelope` writes
 * `v1alpha1` and they would be dropped from the proposal without a word.
 */
export function canvasLoss(
  form: PipelineForm | undefined,
): { key: "noSource" | "noTarget"; field: "source" | "targetEndpoint" } | undefined {
  const drawn = (list: unknown[] | undefined) => (list?.length ?? 0) > 0;
  const source = form?.source?.dataSourceRef !== undefined || form?.source?.endpointRef !== undefined;
  if (!source && (drawn(form?.moreSources) || drawn(form?.processors) || drawn(form?.moreOutputs))) {
    return { key: "noSource", field: "source" };
  }
  if (!form?.targetEndpoint && (drawn(form?.moreOutputs) || drawn(form?.processors))) {
    return { key: "noTarget", field: "targetEndpoint" };
  }
  return undefined;
}

/**
 * The expiry window in words of the person's language, `14d` as "14 days" (PL-65); a window
 * that does not parse is shown as written.
 */
export function expiryWindow(after: string, locale: string): string {
  const match = /^([1-9][0-9]*)([hd])$/.exec(after);
  if (!match) return after;
  return new Intl.NumberFormat(locale, {
    style: "unit",
    unit: match[2] === "d" ? "day" : "hour",
    unitDisplay: "long",
  }).format(Number(match[1]));
}

/** The form one manifest fills, so editing starts from what is in Git rather than from blank. */
/** An output with a type and no write mode is completed with `upsert`, the mode every preset
 * uses, so a typed type never reaches the runner as an unparsable spec (T-0661, PL-32). */
export function completeOutput(form: PipelineForm | undefined): PipelineForm | undefined {
  if (!form?.output?.type || form.output.mode) return form;
  return { ...form, output: { ...form.output, mode: "upsert" } };
}

export function toForm(pipeline: Manifest): PipelineForm {
  const spec = firstShape(pipeline.spec as Spec) as Omit<PipelineForm, "name" | "title" | "source" | "compute"> & {
    source?: Record<string, unknown>;
    compute?: Record<string, unknown>;
    enabled?: boolean;
  };
  const { source, compute, enabled: _enabled, ...rest } = spec;
  void _enabled;
  return prune({
    name: pipeline.metadata.name,
    processors: processorsOf(pipeline.spec as Spec),
    moreSources: moreSourcesOf(pipeline.spec as Spec),
    moreOutputs: moreOutputsOf(pipeline.spec as Spec),
    ...(plainTitle(pipeline.metadata.title) ? { title: plainTitle(pipeline.metadata.title) } : {}),
    ...rest,
    class: typeof rest.class === "string" ? rest.class : "auto",
    source: source ? readSource(source) : undefined,
    compute: compute ? { ...compute, mappingRef: refName(compute.mappingRef) || undefined } : undefined,
  }) as PipelineForm;
}

/** The manifest of a YAML document, read back into the form (the reverse of `toEnvelope`). */
export function fromManifest(document: unknown): PipelineForm {
  const manifest = document as Partial<Manifest>;
  if (!manifest.metadata || typeof manifest.metadata.name !== "string") {
    throw new Error("metadata.name is required");
  }
  return toForm({
    apiVersion: manifest.apiVersion ?? "joinedcontext.com/v1alpha1",
    kind: manifest.kind ?? "Pipeline",
    metadata: manifest.metadata,
    spec: (manifest.spec ?? {}) as Record<string, unknown>,
  });
}

/** Which parts of the form a draft needs on screen (T-2754). */
export function shownFor(form: PipelineForm | undefined): PipelineShown {
  const source = form?.source;
  const query = source?.query ?? {};
  const filled = (value: unknown) =>
    value !== undefined && value !== "" && !(Array.isArray(value) && value.length === 0);
  const temporal = (query.temporalQ as { window?: unknown } | undefined)?.window;
  const compute = (form?.compute ?? {}) as Record<string, unknown>;
  return {
    source:
      source?.dataSourceRef && source.endpointRef
        ? "both"
        : source?.endpointRef
          ? "space"
          : source?.dataSourceRef
            ? "datasource"
            : "none",
    readsMore:
      !source?.endpointRef &&
      (Object.values(query).some(filled) || filled(temporal) || source?.trigger !== undefined),
    computeKind: form?.compute?.kind,
    computeFilled: ["bloblang", "mappingRef", "module", "function"].filter((field) => filled(compute[field])),
    scheduled: form?.class === "scheduled" || filled(form?.schedule),
  };
}

/**
 * The form under the workbench (PL-58): the workbench owns the target and the Bloblang mapping,
 * so the form does not ask them twice. How the rest is grouped and folded is the Pipeline's
 * UiSchema manifest's (UI-02).
 */
export function workbenchUiSchema(base: UiSchema): UiSchema {
  const compute = ((base as Record<string, unknown>).compute ?? {}) as Record<string, unknown>;
  return {
    ...base,
    targetEndpoint: { "ui:widget": "hidden" },
    compute: { ...compute, bloblang: { "ui:widget": "hidden" } },
  } as UiSchema;
}

/**
 * The `bento.yaml` beside a pipeline's manifest (PL-41, T-4818): its text, or `null` when the
 * pipeline has none. Any other answer throws, so a forge that failed is never an empty mapping.
 */
export async function loadMappingFile(project: string, name: string): Promise<string | null> {
  const path = `/api/v1/projects/${encodeURIComponent(project)}/pipelines/${encodeURIComponent(name)}/bento`;
  // A `Request` on this origin, as the model source is read: the session travels with it.
  const response = await globalThis.fetch(
    new Request(`${window.location.origin}${path}`, { headers: { Accept: "text/yaml, text/plain, */*" } }),
  );
  if (response.status === 404) {
    return null;
  }
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  return response.text();
}

/** The known values plus the one already chosen, so an edit never loses its own reference. */
function withCurrent(values: string[], current: string | undefined): string[] {
  return current && !values.includes(current) ? [...values, current] : values;
}

export interface PipelineEditorDialogProps {
  project: string;
  /** Called with `false` when the author closes the dialog; the page unmounts it then. */
  onOpenChange: (open: boolean) => void;
  /** The manifest being edited, or `null` for a new pipeline. */
  editing: Manifest | null;
  /** What a new pipeline's form starts with, when the assistant brought the values (UI-45). */
  initial?: PipelineForm;
  pending: boolean;
  error: string | null;
  /**
   * Proposes the manifest. `files` carries an edited `bento.yaml` beside it (T-4818); the proposal
   * is then the manifest sent with its file rather than the draft, which holds no files.
   */
  onSubmit: (
    envelope: ReturnType<typeof toEnvelope>,
    draft?: { kind: string; name: string },
    files?: Record<string, string>,
  ) => void;
  /** The draft this form edits when the address named one (`?draft=`, AG-61, UI-47). */
  draftName?: string;
  /** Opens the KPI editor over several sources in place of this form (PL-105). */
  onSeveralSources?: () => void;
}

/**
 * One dialog to create or edit a Pipeline: a form and the YAML it writes, both validated
 * against the same schema, proposed through the same change flow as every other write
 * (PL-04, UI-01, AP-13).
 *
 * Mounted only while it is open: the draft starts from the manifest being edited, or blank,
 * and a dialog closed and opened again starts over rather than showing the last draft.
 */
export function PipelineEditorDialog({
  project,
  onOpenChange,
  editing,
  initial,
  pending,
  error,
  onSubmit,
  draftName,
  onSeveralSources,
}: PipelineEditorDialogProps): JSX.Element {
  const { t } = useTranslation();
  const orgDomain = useOrgDomain(project);
  const [draft, setDraft] = useState<PipelineForm | undefined>(() =>
    editing ? toForm(editing) : initial,
  );
  // The document typed in the YAML view is the manifest proposed, not only the fields the form
  // models (T-0885): `enabled: false` typed there is a pause.
  const [typed, setTyped] = useState<Manifest | null>(null);
  const base = typed ?? editing ?? undefined;
  // The last test's answer, tied to the mapping it ran (PL-49): a Bloblang pipeline proposes
  // only while the text in the editor is the text that went green.
  const [verdict, setVerdict] = useState<{
    ok: boolean;
    bloblang: string;
  } | null>(null);
  const bloblang = draft?.compute?.kind === "bloblang" ? (draft.compute.bloblang ?? "") : "";
  // Gated: a pipeline with a Bloblang mapping must pass the test before proposing (PL-49).
  const hasSource =
    draft?.source?.dataSourceRef !== undefined || draft?.source?.endpointRef !== undefined;
  const gate =
    hasSource && bloblang.trim() !== "" && !(verdict?.ok && verdict.bloblang === bloblang)
      ? t("pipelines.test.gate")
      : undefined;
  // Nothing drawn on the canvas is dropped between it and the proposal: Check and Propose both
  // stop and say what is missing (T-3969).
  const loss = canvasLoss(draft);

  // The mapping an edited pipeline with no compute step runs is its `bento.yaml` (PL-41): loaded
  // into step 4, read-only until Edit is on, and proposed with the manifest when changed (T-4818).
  const fileName = editing && !toForm(editing).compute?.kind ? editing.metadata.name : undefined;
  const loadedFile = useQuery({
    queryKey: ["pipeline-bento", project, fileName],
    enabled: fileName !== undefined,
    retry: false,
    refetchOnWindowFocus: false,
    queryFn: () => loadMappingFile(project, fileName ?? ""),
  });
  const [fileEditing, setFileEditing] = useState(false);
  const [fileText, setFileText] = useState<string | null>(null);
  const fileChanged = fileText !== null && fileText !== loadedFile.data;
  const lossRefusal = loss ? { message: t(`pipelines.canvasLoss.${loss.key}`), field: loss.field } : undefined;

  const dataSources = useQuery({
    queryKey: queryKeys.list(project, "datasources"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datasources" } },
        }),
      ),
  });
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const dataSourceList = useMemo(
    () => asManifests(dataSources.data?.items ?? []),
    [dataSources.data],
  );
  const permissions = usePermissions(project);
  const endpointList = useMemo(
    () => reachableEndpoints(permissions.data, asManifests(endpoints.data?.items ?? [])),
    [endpoints.data, permissions.data],
  );
  const dataSourceNames = useMemo(
    () =>
      withCurrent(
        dataSourceList.map((source) => source.metadata.name),
        draft?.source?.dataSourceRef,
      ),
    [dataSourceList, draft?.source?.dataSourceRef],
  );
  const endpointOptions = useMemo<EndpointOption[]>(() => {
    const known = endpointList.map((endpoint) => ({
      name: endpoint.metadata.name,
      urn: endpointUrn(orgDomain, endpoint),
    }));
    const current = draft?.source?.endpointRef;
    const target = draft?.targetEndpoint;
    const names = known.map((endpoint) => endpoint.name);
    const urns = known.flatMap((endpoint) => (endpoint.urn ? [endpoint.urn] : []));
    return [
      ...known,
      ...(current && !names.includes(current) ? [{ name: current }] : []),
      ...(target && !urns.includes(target) ? [{ name: `urn:${target}`, urn: target }] : []),
    ];
  }, [endpointList, orgDomain, draft?.source?.endpointRef, draft?.targetEndpoint]);

  const secretNames = useMemo(() => knownSecretNames(dataSourceList), [dataSourceList]);
  const schema = useMemo(
    () => pipelineSchema(t, dataSourceNames, endpointOptions, secretNames),
    [t, dataSourceNames, endpointOptions, secretNames],
  );
  // Keyed on what decides it, not on the draft: a new arrangement on every keystroke would make
  // RJSF rebuild the input being typed into.
  const shownKey = JSON.stringify(shownFor(draft));
  const uiSchema = useMemo(
    () => workbenchUiSchema(pipelineUiSchemaFor(JSON.parse(shownKey) as PipelineShown)),
    [shownKey],
  );
  const source = useMemo<ManifestSource<PipelineForm>>(
    () => ({
      toManifest: (form) => toEnvelope(project, form, base),
      fromManifest: (document) => {
        const form = fromManifest(document);
        setTyped(document as Manifest);
        return form;
      },
    }),
    [project, base],
  );

  const mappingFile: MappingFile | undefined =
    fileName === undefined
      ? undefined
      : {
          state: loadedFile.isPending
            ? "loading"
            : loadedFile.isError
              ? "failed"
              : loadedFile.data === null
                ? "absent"
                : "loaded",
          text: fileText ?? loadedFile.data ?? "",
          editing: fileEditing,
          onEditing: setFileEditing,
          onText: setFileText,
          failure: (
            <ListFailed
              what="bento.yaml"
              reason={reasonOf(loadedFile.error, t("form.invalid"))}
              onRetry={() => void loadedFile.refetch()}
            />
          ),
        };

  // The mapping is `compute.bloblang` in the form (PL-41); left empty it stays in `bento.yaml`
  // beside the manifest in the forge (Architecture/08 §3), so the dialog links there too.
  const bentoUrl = editing?.status?.sourceUrl?.replace(/pipeline\.yaml$/, "bento.yaml");
  const bentoFileLink = safeHref(bentoUrl) ? (
    <a
      href={safeHref(bentoUrl)}
      target="_blank"
      rel="noreferrer"
      className={buttonClass("ghost", "sm", "text-primary-soft-fg")}
    >
      {t("pipelines.bloblangFile")}
      <Icon name="external" className="size-3.5" />
    </a>
  ) : undefined;

  return (
    <ResourceFormDialog<PipelineForm>
      open
      onOpenChange={onOpenChange}
      title={editing ? t("pipelines.dialog.edit") : t("pipelines.dialog.create")}
      description={t("pipelines.dialog.lead")}
      schema={schema}
      // A rename is a new manifest at a new path, so the name is fixed once it exists.
      uiSchema={uiSchema}
      lockedName={editing?.metadata.name}
      formData={draft}
      project={project}
      // The form edits a Portal draft, so a second window and the assistant see the same text
      // and Propose carries the draft it edited (AG-61, UI-47, T-0791).
      draftKind="Pipeline"
      draftName={editing?.metadata.name ?? draftName}
      // The proposal names the draft, so under strict validation it needs a green verdict: the
      // dialog runs the kind's dry run itself (T-0779, PF-57).
      plural="pipelines"
      submitLabel={t("pipelines.propose")}
      disabled={pending}
      submitDisabledReason={lossRefusal?.message ?? gate}
      checkRefusal={lossRefusal}
      error={error}
      source={source}
      onChange={(form) => setDraft(completeOutput(form))}
      onSubmit={(form, draftRef) =>
        fileChanged
          ? onSubmit(toEnvelope(project, form, base), undefined, { "bento.yaml": fileText ?? "" })
          : onSubmit(toEnvelope(project, form, base), draftRef)
      }
      // The check runs by itself once typing rests, so Propose opens without a hidden step (T-4818).
      autoCheck
      afterFields={
        <>
          {editing ? null : (
            <RecipeGallery
              project={project}
              orgDomain={orgDomain}
              dataSources={dataSourceList}
              endpoints={endpointList}
              targets={endpointOptions}
              onUse={(form) => setDraft(completeOutput(form))}
              onSeveralSources={onSeveralSources}
            >
              <TemplateList project={project} onUse={(form) => setDraft(completeOutput(form))} />
            </RecipeGallery>
          )}
          <PipelineWorkbench
            project={project}
            draft={draft}
            onChange={(form) => setDraft(completeOutput(form))}
            dataSources={dataSourceList}
            endpoints={endpointList}
            targets={endpointOptions}
            toManifest={source.toManifest}
            onVerdict={(ok, tested) => setVerdict({ ok, bloblang: tested })}
            mappingFile={mappingFile}
            draftName={editing?.metadata.name ?? draftName}
            pipeline={editing?.metadata.name}
          >
            {/* What the six steps do not cover (several sources, processor steps, a KPI, the flow
                view) stays one fold away, on the same draft (ADR-N-034). */}
            <details className="rounded-lg border border-border p-3">
              <summary className="cursor-pointer text-body font-semibold text-fg">
                {t("pipelines.workbench.advanced")}
                <span className="ml-2 font-normal text-caption text-fg-muted">
                  {t("pipelines.workbench.advancedHint")}
                </span>
              </summary>
              <div className="mt-3">
                <PipelineStudio
                  project={project}
                  draft={draft}
                  onChange={(form) => setDraft(completeOutput(form))}
                  dataSources={dataSourceList}
                  endpoints={endpointList}
                  toManifest={source.toManifest}
                  onVerdict={(ok, tested) => setVerdict({ ok, bloblang: tested })}
                />
              </div>
            </details>
          </PipelineWorkbench>
          {draft?.compute?.kind === "bloblang" ? (
            <Alert tone="info" actions={bentoFileLink}>
              {t("pipelines.bloblangHint")}
            </Alert>
          ) : null}
          <SaveAsTemplate project={project} manifest={() => toEnvelope(project, draft ?? {}, base)} />
        </>
      }
    >
      {dataSources.isError ? (
        <ListFailed
          what={t("nav.datasources")}
          reason={reasonOf(dataSources.error, t("form.invalid"))}
          onRetry={() => void dataSources.refetch()}
        />
      ) : null}
      {endpoints.isError ? (
        <ListFailed
          what={t("nav.endpoints")}
          reason={reasonOf(endpoints.error, t("form.invalid"))}
          onRetry={() => void endpoints.refetch()}
        />
      ) : null}
    </ResourceFormDialog>
  );
}

export { PLURAL as PIPELINES_PLURAL };
