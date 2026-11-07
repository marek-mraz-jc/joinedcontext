import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import type { ErrorSchema } from "@rjsf/utils";
import { SchemaForm } from "../../components/forms/SchemaForm";
import { AttributePicker } from "../../components/pickers/AttributePicker";
import { TypePicker } from "../../components/pickers/TypePicker";
import { CodeBlock, Button, Field, Input, Select, Tabs, tabPanelProps, Textarea } from "../../components/ui";
import processorCatalogue from "../../schemas/bento-processors.json";
import { COMPUTE_KINDS, OUTPUT_MODES, YamlFieldError } from "../../schemas/kinds";
import type { OutputForm, PipelineForm, RouteForm, SourceForm, StepForm } from "./PipelineEditor";
import { spaceOf } from "../spaces/spaceFacts";
import { sourceKindOf } from "./PipelineStudio";
import type { Trace } from "./PipelineTest";
import { errorAt, fromFormData, toFormData, useProcessorForms, withHelp, withVariables } from "./processorForm";
import { connect, laneOf, missingOf, move } from "./pipelineGraph";
import { wireCount, wireKey } from "./pipelineDebug";
import type { WireRefusal } from "./pipelineGraph";

/** One processor of the pinned runner: the list jc-core admits a `processor` step from (PL-52). */
export interface Processor {
  name: string;
  category: string;
  summary: string;
}

/** The runner's processors by category, in the catalogue's order (PL-56). */
export const PROCESSOR_GROUPS: { category: string; processors: Processor[] }[] = (() => {
  const groups = new Map<string, Processor[]>();
  for (const processor of processorCatalogue as Processor[]) {
    groups.set(processor.category, [...(groups.get(processor.category) ?? []), processor]);
  }
  return [...groups].map(([category, processors]) => ({ category, processors }));
})();

const PROCESSOR_NAMES = new Set((processorCatalogue as Processor[]).map(({ name }) => name));
const PROCESSOR_COUNT = PROCESSOR_NAMES.size;

/** The runner's summary as plain words: its Markdown links keep their text and lose the target. */
export const plainSummary = (summary: string): string =>
  summary.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");

/**
 * A step node is `step-<index into form.processors>`, a second source `source-<index into
 * form.moreSources>` and a second output `output-<index into form.moreOutputs>`; the first
 * source, the compute step and the first output are what they were, so everything the studio
 * hangs off `"source"` still finds it. A route's step is `route-<route>-step-<index>` and its
 * output `route-<route>-output-<index>`, both into `form.routes` (PL-73).
 */
export type FlowNodeId =
  | "source"
  | "compute"
  | "output"
  | `step-${number}`
  | `source-${number}`
  | `output-${number}`
  | `route-${number}-step-${number}`
  | `route-${number}-output-${number}`;

export const stepIndexOf = (id: FlowNodeId | null): number | undefined =>
  id?.startsWith("step-") ? Number(id.slice(5)) : undefined;

export const sourceIndexOf = (id: FlowNodeId | null): number | undefined =>
  id?.startsWith("source-") ? Number(id.slice(7)) : undefined;

const isSource = (id: FlowNodeId): boolean => id === "source" || id.startsWith("source-");

export const outputIndexOf = (id: FlowNodeId | null): number | undefined =>
  id?.startsWith("output-") ? Number(id.slice(7)) : undefined;

/** Which route a node belongs to, and which of its steps or outputs it is (PL-73). */
export function routeNodeOf(
  id: FlowNodeId | null,
): { route: number; step?: number; output?: number } | undefined {
  const match = id ? /^route-(\d+)-(step|output)-(\d+)$/.exec(id) : null;
  if (!match) return undefined;
  const route = Number(match[1]);
  return match[2] === "step" ? { route, step: Number(match[3]) } : { route, output: Number(match[3]) };
}

const isOutput = (id: FlowNodeId): boolean =>
  id === "output" || id.startsWith("output-") || routeNodeOf(id)?.output !== undefined;

/** A node of the lane: neither a source, an output, nor a step of a route. */
export const isLane = (id: FlowNodeId): boolean =>
  !isSource(id) && !isOutput(id) && routeNodeOf(id) === undefined;

export interface FlowNode {
  id: FlowNodeId;
  kind: string;
  label: string;
  summary: string;
  present: boolean;
}

export interface FlowEdge {
  from: FlowNodeId;
  to: FlowNodeId;
  /** The route whose condition this wire carries: the wire from the lane into a route (PL-73). */
  route?: number;
}

/** The one processor a `processor` step names, or nothing for a compute or `mappingRef` step. */
export function processorOf(step: StepForm["step"]): [string, unknown] | undefined {
  const { processor } = step;
  if (typeof processor !== "object" || processor === null) return undefined;
  const [entry] = Object.entries(processor);
  return entry;
}

function stepNode(entry: StepForm, index: number): FlowNode {
  const processor = processorOf(entry.step);
  const config = processor?.[1];
  const summary =
    typeof config === "string"
      ? (config.trim().split("\n")[0] ?? "")
      : config && typeof config === "object"
        ? Object.keys(config).join(" · ")
        : "";
  const kind = processor?.[0] ?? (typeof entry.step.kind === "string" ? entry.step.kind : "mapping");
  return { id: `step-${index}`, kind, label: "Step", summary, present: true };
}

/** What a source reads, in the words the canvas has room for. */
function sourceNodeOf(source: SourceForm | undefined, id: FlowNodeId): FlowNode {
  const ref =
    source?.dataSourceRef ||
    (typeof source?.endpointRef === "object"
      ? ((source.endpointRef as { name?: string })?.name ?? "")
      : source?.endpointRef) ||
    "";
  const queryType = (source?.query?.type as string | undefined) ?? "";
  return {
    id,
    kind: source?.dataSourceRef ? "datasource" : source?.endpointRef ? "space" : "none",
    label: "Source",
    summary: [ref, queryType].filter(Boolean).join(" · "),
    present: true,
  };
}

export function toFlow(form: PipelineForm | undefined): { nodes: FlowNode[]; edges: FlowEdge[] } {
  // The first source keeps `sourceKindOf`, which is the kind the studio's own source section
  // chose and can be `none` while nothing is picked yet.
  const sourceNode: FlowNode = { ...sourceNodeOf(form?.source, "source"), kind: sourceKindOf(form) };
  const moreSourceNodes = (form?.moreSources ?? []).map((source, index) =>
    sourceNodeOf(source, `source-${index}`),
  );

  const targetName = form?.targetEndpoint ? form.targetEndpoint.split(":").pop() ?? "" : "";
  const outputSummary = [form?.output?.type, form?.output?.mode, targetName]
    .filter(Boolean)
    .join(" · ");

  const outputNode: FlowNode = {
    id: "output",
    kind: "output",
    label: "Output",
    summary: outputSummary,
    present: true,
  };
  const moreOutputNodes = (form?.moreOutputs ?? []).map(
    (output, index): FlowNode => ({
      id: `output-${index}`,
      kind: "output",
      label: "Output",
      summary: [output.type, output.mode, output.targetEndpoint?.split(":").pop()]
        .filter(Boolean)
        .join(" · "),
      present: true,
    }),
  );

  let computeNode: FlowNode | undefined;
  if (form?.compute?.kind) {
    let computeSummary = "";
    if (form.compute.kind === "bloblang") {
      computeSummary = (form.compute.bloblang ?? "").trim().split("\n")[0] ?? "";
    } else if (form.compute.kind === "mapping") {
      computeSummary = form.compute.mappingRef ?? "";
    } else if (form.compute.kind === "wasm" || form.compute.kind === "container") {
      computeSummary = [form.compute.module, form.compute.function].filter(Boolean).join(".");
    }
    computeNode = {
      id: "compute",
      kind: form.compute.kind,
      label: "Compute",
      summary: computeSummary,
      present: true,
    };
  }

  // One lane, in the order the steps run (PL-56): the manifest stores no edges, so an edge is
  // nothing but two neighbours.
  const steps = (form?.processors ?? []).map((entry, index) => ({
    entry,
    node: stepNode(entry, index),
  }));
  const lane = [
    ...steps.filter(({ entry }) => !entry.after).map(({ node }) => node),
    ...(computeNode ? [computeNode] : []),
    ...steps.filter(({ entry }) => entry.after).map(({ node }) => node),
  ];
  const sources = [sourceNode, ...moreSourceNodes];
  const outputs = [outputNode, ...moreOutputNodes];
  // Every source feeds the head of the lane, which is how the runner merges them (PL-53), and
  // its tail feeds every output, which is how it fans out (PL-55): a `broker` is not a node an
  // author places, so the canvas draws none.
  const tail = lane.length > 0 ? [lane[lane.length - 1]] : sources;
  const edges: FlowEdge[] = [
    ...(lane.length > 0 ? sources.map((source) => ({ from: source.id, to: lane[0].id })) : []),
    ...lane.slice(1).map((node, index) => ({ from: lane[index].id, to: node.id })),
    ...tail.flatMap((from) => outputs.map((output) => ({ from: from.id, to: output.id }))),
  ];
  // A route leaves the lane's tail on a wire that carries its condition, runs its own steps and
  // ends at its own outputs (PL-73). The manifest still stores no edges: this is its list order.
  const routeNodes = (form?.routes ?? []).flatMap((route, at) => {
    const steps = (route.steps ?? []).map(
      (step, index): FlowNode => ({ ...stepNode({ step }, index), id: `route-${at}-step-${index}` }),
    );
    const ends = (route.outputs ?? []).map(
      (output, index): FlowNode => ({
        id: `route-${at}-output-${index}`,
        kind: "output",
        label: "Output",
        summary: [route.name, output.type, output.targetEndpoint?.split(":").pop()]
          .filter(Boolean)
          .join(" · "),
        present: true,
      }),
    );
    const entry = steps.length > 0 ? [steps[0]] : ends;
    edges.push(
      ...tail.flatMap((from) => entry.map((to) => ({ from: from.id, to: to.id, route: at }))),
      ...steps.slice(1).map((node, index) => ({ from: steps[index].id, to: node.id })),
      ...(steps.length > 0 ? ends.map((end) => ({ from: steps[steps.length - 1].id, to: end.id })) : []),
    );
    return [...steps, ...ends];
  });
  return { nodes: [...sources, ...lane, ...outputs, ...routeNodes], edges };
}

/** The first name `route-<n>` no route of the form holds. */
function freeRouteName(form: PipelineForm): string {
  const taken = new Set((form.routes ?? []).map((route) => route.name));
  let n = 1;
  while (taken.has(`route-${n}`)) n += 1;
  return `route-${n}`;
}

/**
 * The form with one more route (PL-73): no condition yet, and one output to pick an Endpoint
 * for, because every route ends in an output. The node it became is that output.
 */
export function addRoute(form: PipelineForm | undefined): { form: PipelineForm; id: FlowNodeId } {
  const base: PipelineForm = form ?? { class: "auto" };
  const routes = [...(base.routes ?? []), { name: freeRouteName(base), when: "", outputs: [{ mode: "upsert" }] }];
  return { form: { ...base, routes }, id: `route-${routes.length - 1}-output-0` };
}

/** The form without route `index`. */
export function removeRoute(form: PipelineForm, index: number): PipelineForm {
  return { ...form, routes: (form.routes ?? []).filter((_, at) => at !== index) };
}

/**
 * The form without node `id` of a route: a step goes, and so does an output, except the route's
 * last one, which takes the route with it, because a route without an output writes nowhere.
 */
export function removeRouteNode(form: PipelineForm, id: FlowNodeId): PipelineForm {
  const node = routeNodeOf(id);
  const route = node && form.routes?.[node.route];
  if (!node || !route) return form;
  if (node.output !== undefined && (route.outputs ?? []).length <= 1) return removeRoute(form, node.route);
  const changed = {
    ...route,
    steps: (route.steps ?? []).filter((_, at) => at !== node.step),
    outputs: (route.outputs ?? []).filter((_, at) => at !== node.output),
  };
  return { ...form, routes: (form.routes ?? []).map((was, at) => (at === node.route ? changed : was)) };
}

/** The form with one more source to read, and the node it became (PL-52). */
export function addSource(form: PipelineForm | undefined): { form: PipelineForm; id: FlowNodeId } {
  const base: PipelineForm = form ?? { class: "auto" };
  const more = [...(base.moreSources ?? []), {}];
  return { form: { ...base, moreSources: more }, id: `source-${more.length - 1}` };
}

/**
 * The form without the second source `index`. The first source is the one the sample, the KPI
 * preset and the access panel read, so it is never removed here — the source section clears it.
 */
export function removeSource(form: PipelineForm, index: number): PipelineForm {
  return { ...form, moreSources: (form.moreSources ?? []).filter((_, at) => at !== index) };
}

/** The form with one more output, empty until an Endpoint is picked, and the node it became. */
export function addOutput(form: PipelineForm | undefined): { form: PipelineForm; id: FlowNodeId } {
  const base: PipelineForm = form ?? { class: "auto" };
  const more = [...(base.moreOutputs ?? []), { mode: "upsert" }];
  return { form: { ...base, moreOutputs: more }, id: `output-${more.length - 1}` };
}

/** The form without the second output `index`; the first output is what the pipeline writes. */
export function removeOutput(form: PipelineForm, index: number): PipelineForm {
  return { ...form, moreOutputs: (form.moreOutputs ?? []).filter((_, at) => at !== index) };
}

/**
 * What a processor starts as. The runner refuses `mapping: {}`: a mapping is text and a
 * composition is a list, so each starts as the smallest block its own shape lints green.
 */
function blankConfig(name: string): unknown {
  if (["mapping", "mutation", "bloblang"].includes(name)) return "root = this";
  if (["catch", "try", "for_each", "switch", "group_by", "parallel"].includes(name)) return [];
  return {};
}

/**
 * The form with processor `name` put behind node `after` (PL-56), and the node it became.
 * Behind the source is the head of the lane; behind the output, or behind nothing, is its tail,
 * because nothing runs after an output. A name the runner does not ship changes nothing.
 */
export function addStep(
  form: PipelineForm | undefined,
  after: FlowNodeId | null,
  name: string,
): { form: PipelineForm; id: FlowNodeId } | undefined {
  if (!PROCESSOR_NAMES.has(name)) return undefined;
  const base: PipelineForm = form ?? { class: "auto" };
  // Into a route: behind the route's step, or at the end of its steps from one of its outputs.
  const inRoute = routeNodeOf(after);
  const route = inRoute && base.routes?.[inRoute.route];
  if (inRoute && route) {
    const own = [...(route.steps ?? [])];
    const index = inRoute.step !== undefined ? inRoute.step + 1 : own.length;
    own.splice(index, 0, { processor: { [name]: blankConfig(name) } });
    return {
      form: { ...base, routes: (base.routes ?? []).map((was, at) => (at === inRoute.route ? { ...route, steps: own } : was)) },
      id: `route-${inRoute.route}-step-${index}`,
    };
  }
  const steps = base.processors ?? [];
  const head = steps.filter((entry) => !entry.after);
  const tail = steps.filter((entry) => entry.after);
  const step = { processor: { [name]: blankConfig(name) } };
  const at = stepIndexOf(after);
  let index: number;
  if (after === "source") {
    index = 0;
    head.unshift({ step });
  } else if (after === "compute" || (at !== undefined && steps[at]?.after)) {
    // Behind the compute step, or behind a step that already runs after it.
    index = after === "compute" ? head.length : (at as number) + 1;
    tail.splice(index - head.length, 0, { step, after: true });
  } else if (at !== undefined && at < head.length) {
    index = at + 1;
    head.splice(index, 0, { step });
  } else if (base.compute?.kind || tail.length > 0) {
    index = steps.length;
    tail.push({ step, after: true });
  } else {
    index = head.length;
    head.push({ step });
  }
  return { form: { ...base, processors: [...head, ...tail] }, id: `step-${index}` };
}

/** The form without step `index`. The list stays, empty, so the save drops the step too. */
export function removeStep(form: PipelineForm, index: number): PipelineForm {
  return { ...form, processors: (form.processors ?? []).filter((_, at) => at !== index) };
}

export function setComputeKind(form: PipelineForm | undefined, kind: string | null): PipelineForm {
  const base: PipelineForm = form ? { ...form } : { class: "auto" };
  if (!kind) {
    const rest: PipelineForm = { ...base };
    delete rest.compute;
    return rest;
  }
  const prev = base.compute;
  const compute: NonNullable<PipelineForm["compute"]> = { kind };
  if (kind === "bloblang") {
    if (prev?.bloblang) compute.bloblang = prev.bloblang;
  } else if (kind === "mapping") {
    if (prev?.mappingRef) compute.mappingRef = prev.mappingRef;
  } else if (kind === "wasm" || kind === "container") {
    if (prev?.module) compute.module = prev.module;
    if (prev?.function) compute.function = prev.function;
  }
  return { ...base, compute };
}

export interface NodePaint {
  eventsIn?: number;
  eventsOut?: number;
  error?: string;
  state: "ok" | "error" | "skipped" | "idle";
}

export function paintOf(trace: Trace | null, nodes: FlowNode[]): Record<string, NodePaint> {
  const result: Record<string, NodePaint> = {
    source: { state: "idle" },
    compute: { state: "idle" },
    output: { state: "idle" },
  };
  for (const node of nodes) result[node.id] = { state: "idle" };

  if (!trace) {
    return result;
  }

  let sourceError: string | undefined;
  let computeError: string | undefined;
  let outputError: string | undefined;
  // A mapping error that names its step (PL-43) paints that step and nothing else; the lane's
  // steps are `form.processors` and the compute step, in the order the manifest writes them.
  const stepErrors = new Map<number, string>();

  for (const err of trace.errors ?? []) {
    const stage = (err.stage || "").toLowerCase();
    if (stage.includes("mapping") || stage.includes("compute") || stage.includes("bloblang")) {
      if (typeof err.step === "number" && err.step >= 0) {
        if (!stepErrors.has(err.step)) stepErrors.set(err.step, err.message);
      } else if (!computeError) {
        computeError = err.message;
      }
    } else if (stage.includes("input") || stage.includes("source")) {
      if (!sourceError) sourceError = err.message;
    } else if (stage.includes("validation") || stage.includes("output")) {
      if (!outputError) outputError = err.message;
    } else {
      if (!computeError) computeError = err.message;
    }
  }

  if (!outputError) {
    const failedVal = trace.validation?.find((v) => !v.ok);
    if (failedVal) {
      outputError =
        failedVal.problems && failedVal.problems.length > 0
          ? failedVal.problems[0]
          : "validation failed";
    }
  }

  // A lane without a compute step still has somewhere a mapping can fail: its first step.
  const failing =
    nodes.find((node) => node.id === "compute") ??
    nodes.find((node) => stepIndexOf(node.id) !== undefined);
  // The trace has one input stage however many sources feed it, so each of them shows it.
  const sourceNodes = nodes.filter((node) => isSource(node.id));
  const read = trace.input?.events ?? 0;
  // The lane in manifest order, so a step index from the trace names the node that ran it.
  const lane = nodes.filter((node) => isLane(node.id));
  const errorsByNode: Record<string, string | undefined> = {
    ...Object.fromEntries(sourceNodes.map((node) => [node.id, sourceError])),
    ...Object.fromEntries(
      [...stepErrors].flatMap(([at, message]) => (lane[at] ? [[lane[at].id, message]] : [])),
    ),
    output: outputError,
  };
  // A mapping error that named no step, or one the lane has no node for, still has to be said:
  // it goes where it went before, on the step that would have run it.
  const unplaced =
    computeError ??
    [...stepErrors].find(([at]) => !lane[at])?.[1];
  if (unplaced && failing && !errorsByNode[failing.id]) {
    errorsByNode[failing.id] = unplaced;
  }

  const eventsInByNode: Record<string, number | undefined> = {
    ...Object.fromEntries(sourceNodes.map((node) => [node.id, read])),
    compute: read,
    output: trace.mapping?.length ?? 0,
  };

  const eventsOutByNode: Record<string, number | undefined> = {
    ...Object.fromEntries(sourceNodes.map((node) => [node.id, read])),
    compute: trace.mapping?.length ?? 0,
    output: (trace.validation ?? []).filter((v) => v.ok).length,
  };

  // The sources are one stage however many there are: the trace carries a single input stage and
  // does not say which source it came from, so a failed read marks them all rather than claiming
  // one failed and the others never ran. Behind them the lane stops at its first error.
  let errorEncountered = Boolean(sourceError);
  for (const node of nodes) {
    // Every output writes the same messages (PL-55): a second one is painted as the first.
    if (outputIndexOf(node.id) !== undefined) continue;
    const key = node.id;
    const err = errorsByNode[key];
    let state: NodePaint["state"] = "ok";
    if (isSource(node.id)) {
      state = err ? "error" : "ok";
    } else if (errorEncountered) {
      state = "skipped";
    } else if (err) {
      state = "error";
      errorEncountered = true;
    }
    result[node.id] = {
      eventsIn: eventsInByNode[key],
      eventsOut: eventsOutByNode[key],
      error: err,
      state,
    };
  }
  for (const node of nodes) {
    if (outputIndexOf(node.id) !== undefined) result[node.id] = { ...result.output };
  }

  return result;
}

/**
 * What a node took in and gave out in a test (PL-67): a lane node reads the step before it and
 * shows its own stage, the sources show what was read, an output what passed the model. A trace
 * without `stages` (an older harness) gives what the drawer showed before: the read for the
 * sources and the compute step, the mapped output for the rest.
 */
export function samplesOf(
  trace: Trace,
  nodes: FlowNode[],
  id: FlowNodeId,
): { in?: unknown; out?: unknown } {
  const read = trace.input?.sample;
  const passed = (() => {
    const ok = trace.validation?.find((verdict) => verdict.ok);
    return ok !== undefined ? trace.mapping?.[ok.index] : undefined;
  })();
  if (isSource(id)) return { in: read, out: read };
  const stages = trace.stages;
  if (!stages) {
    if (id === "compute") return { in: read, out: trace.mapping?.[0] };
    return { in: trace.mapping?.[0], out: passed };
  }
  const lane = nodes.filter((node) => isLane(node.id));
  const sampleAt = (at: number) => (at < 0 ? read : stages.find((stage) => stage.step === at)?.sample);
  if (isOutput(id)) return { in: sampleAt(lane.length - 1), out: passed };
  const at = lane.findIndex((node) => node.id === id);
  return at < 0 ? {} : { in: sampleAt(at - 1), out: sampleAt(at) };
}

/** What one component of the running stream counted, by its label (API/01 §7, PL-66). */
export type NodeCounters = Record<
  string,
  { received?: number | null; sent?: number | null; errors?: number | null }
>;

/**
 * The runner's label of a node (PL-66): `input` for the sources, `step_{j}` for the `j`-th
 * processor step, `compute`, and `output` for the fan-out, which is one broker whose outputs
 * are not counted one by one.
 */
export function liveLabelOf(id: FlowNodeId): string {
  if (isSource(id)) return "input";
  if (isOutput(id)) return "output";
  const step = stepIndexOf(id);
  return step !== undefined ? `step_${step}` : id;
}

/** What one node counted in each window of the stats route (API/01 §7, PL-68). */
type Counted = { received?: number | null; sent?: number | null; errors?: number | null };
export type NodeWindows = Record<string, { "1m": Counted; "5m": Counted; "15m": Counted }>;

/**
 * The canvas' live numbers from the stats route: received and sent in the last minute, and the
 * errors of the last five, so a node turns red for five minutes after a failure, not for ever.
 */
export function countersOfStats(nodes: NodeWindows): NodeCounters {
  return Object.fromEntries(
    Object.entries(nodes).map(([label, windows]) => [
      label,
      { received: windows["1m"]?.received, sent: windows["1m"]?.sent, errors: windows["5m"]?.errors },
    ]),
  );
}

/**
 * The flow painted from the running stream (PL-66): each node with the counters of its own
 * label ([`liveLabelOf`]). A node with errors is failing; one the runner reports nothing for
 * is idle. `errorsSentence` says the count in the page's language.
 */
export function paintOfCounters(
  counters: NodeCounters,
  nodes: FlowNode[],
  errorsSentence: (count: number) => string,
): Record<string, NodePaint> {
  return Object.fromEntries(
    nodes.map((node) => {
      const counted = counters[liveLabelOf(node.id)];
      if (!counted) return [node.id, { state: "idle" } satisfies NodePaint];
      const errors = counted.errors ?? 0;
      const paint: NodePaint = {
        eventsIn: counted.received ?? undefined,
        eventsOut: counted.sent ?? undefined,
        state: errors > 0 ? "error" : "ok",
        error: errors > 0 ? errorsSentence(errors) : undefined,
      };
      return [node.id, paint];
    }),
  );
}

/**
 * The messages a running stream holds before its output, amber once past this many (T-3973).
 * ponytail: one default for every pipeline; a per-pipeline threshold when a manifest declares one.
 */
export const QUEUE_WARN = 100;

/** The buffer depth the metrics route answers, as the wire into the output shows it (T-3973). */
export function queueOf(depth: number | null | undefined): { depth: number; high: boolean } | undefined {
  return typeof depth === "number" && depth >= 0 ? { depth, high: depth > QUEUE_WARN } : undefined;
}

export interface PipelineFlowProps {
  form: PipelineForm | undefined;
  onChange: (f: PipelineForm) => void;
  trace: Trace | null;
  /** The running stream's counters per node; painted whenever no test result is on screen. */
  live?: NodeCounters;
  /** The running stream's buffer depth (`bufferDepth`), drawn on the wire into the output. */
  bufferDepth?: number | null;
  selected: FlowNodeId | null;
  onSelect: (id: FlowNodeId | null) => void;
  /** Opens a node's form: a double-click or Enter on the node, Open in the list (PL-68). */
  onOpen?: (id: FlowNodeId) => void;
  /** The wires a Debug node taps, by `wireKey`, and the toggle of one (T-3222). */
  taps?: string[];
  onTap?: (key: string) => void;
  dataSources: Manifest[];
  endpoints: Manifest[];
}

export function PipelineFlow({
  form,
  onChange,
  trace,
  live,
  bufferDepth,
  selected,
  onSelect,
  onOpen,
  taps = [],
  onTap,
}: PipelineFlowProps): JSX.Element {
  const { t } = useTranslation();
  const { nodes, edges } = toFlow(form);
  const open = onOpen ?? onSelect;
  const queue = queueOf(bufferDepth);
  // Undo and redo of the graph's own edits (PL-68). An edit made elsewhere (a node's form, the
  // YAML view) is not the graph's to take back, so it ends the history instead of being undone.
  const [history, setHistory] = useState<{ past: PipelineForm[]; future: PipelineForm[]; at: string }>({
    past: [],
    future: [],
    at: "",
  });
  const now = JSON.stringify(form ?? null);
  const own = history.at === now;
  const past = own ? history.past : [];
  const future = own ? history.future : [];
  const change = (next: PipelineForm) => {
    setHistory({ past: [...past, form ?? { class: "auto" }].slice(-50), future: [], at: JSON.stringify(next) });
    onChange(next);
  };
  const undo = () => {
    const previous = past[past.length - 1];
    if (!previous) return;
    setHistory({ past: past.slice(0, -1), future: [form ?? { class: "auto" }, ...future], at: JSON.stringify(previous) });
    onChange(previous);
  };
  const redo = () => {
    const [next, ...rest] = future;
    if (!next) return;
    setHistory({ past: [...past, form ?? { class: "auto" }], future: rest, at: JSON.stringify(next) });
    onChange(next);
  };
  // `null` fits the canvas to the width it has; a number is a zoom the person chose.
  const [zoom, setZoom] = useState<number | null>(null);
  const [view, setView] = useState<"canvas" | "list">("canvas");
  const [query, setQuery] = useState("");
  const [wiring, setWiring] = useState<{ from: FlowNodeId; x: number; y: number } | null>(null);
  const [said, setSaid] = useState<string | null>(null);
  const wire = (from: FlowNodeId, to: FlowNodeId) => {
    const result = connect(form, from, to);
    if ("refused" in result) {
      setSaid(t(`pipelines.flow.wire.${result.refused satisfies WireRefusal}`));
    } else if ("form" in result) {
      change(result.form);
      onSelect(result.id);
      setSaid(t("pipelines.flow.wire.moved"));
    } else {
      setSaid(t("pipelines.flow.wire.already"));
    }
  };
  const shift = (id: FlowNodeId, by: -1 | 1) => {
    const moved = move(form, id, by);
    if (moved) {
      change(moved.form);
      onSelect(moved.id);
    }
  };
  const lane = laneOf(form);
  const missing: Record<string, string | undefined> = Object.fromEntries(
    nodes.map((node) => [node.id, missingOf(form, node.id)]),
  );
  const stateWords = (id: FlowNodeId): string =>
    missing[id] ? t(`pipelines.flow.state.missing.${missing[id]}`) : t("pipelines.flow.state.configured");
  const nodeName = (node: FlowNode): string => {
    const inRoute = routeNodeOf(node.id);
    if (inRoute) {
      const name = form?.routes?.[inRoute.route]?.name ?? "";
      return t(`pipelines.flow.node.${inRoute.step !== undefined ? "routeStep" : "routeOutput"}`, { name });
    }
    return t(
      `pipelines.flow.node.${
        stepIndexOf(node.id) !== undefined
          ? "step"
          : sourceIndexOf(node.id) !== undefined
            ? "source"
            : outputIndexOf(node.id) !== undefined
              ? "output"
              : node.id
      }`,
      { defaultValue: node.label },
    );
  };
  const needle = query.trim().toLowerCase();
  const found = needle
    ? (processorCatalogue as Processor[]).filter(
        ({ name, category, summary }) =>
          name.toLowerCase().includes(needle) ||
          category.toLowerCase().includes(needle) ||
          summary.toLowerCase().includes(needle),
      )
    : [];
  // A test result is what the author just asked for; without one, the running stream speaks.
  const showsLive = !trace && live !== undefined && Object.keys(live).length > 0;
  const paint = showsLive
    ? paintOfCounters(live, nodes, (count) => t("pipelines.flow.liveErrors", { count }))
    : paintOf(trace, nodes);

  const nodeWidth = 200;
  const nodeHeight = 100;
  const nodeSpacing = 240;
  const rowSpacing = 120;
  const startX = 20;
  const startY = 25;

  // Sources stand in the left column and outputs in the right one, each one under the other,
  // and the lane runs between them along the middle of the taller column (PL-56). Each route
  // takes rows of its own below, one per output, its steps under the lane's (PL-73).
  const sourceCount = nodes.filter((node) => isSource(node.id)).length;
  const outputCount = nodes.filter((node) => isOutput(node.id) && !routeNodeOf(node.id)).length;
  const laneCount = nodes.filter((node) => isLane(node.id)).length;
  const routeSteps = Math.max(0, ...(form?.routes ?? []).map((route) => route.steps?.length ?? 0));
  const columnHeight = (Math.max(sourceCount, outputCount) - 1) * rowSpacing;
  const column = (count: number, at: number) =>
    startY + (columnHeight - (count - 1) * rowSpacing) / 2 + at * rowSpacing;
  const outputX = startX + (laneCount + 1 + routeSteps) * nodeSpacing;
  const routeRows: number[] = [];
  let routeRowCount = 0;
  for (const route of form?.routes ?? []) {
    routeRows.push(routeRowCount);
    routeRowCount += Math.max(1, route.outputs?.length ?? 0);
  }
  const routeTop = startY + columnHeight + rowSpacing;
  const placed = nodes.map((node, index) => {
    const inRoute = routeNodeOf(node.id);
    if (inRoute) {
      const y = routeTop + (routeRows[inRoute.route] ?? 0) * rowSpacing;
      return inRoute.step !== undefined
        ? { node, x: startX + (laneCount + 1 + inRoute.step) * nodeSpacing, y }
        : { node, x: outputX, y: y + (inRoute.output ?? 0) * rowSpacing };
    }
    if (isSource(node.id)) return { node, x: startX, y: column(sourceCount, index) };
    if (isOutput(node.id)) {
      const at = index - sourceCount - laneCount;
      return { node, x: outputX, y: column(outputCount, at) };
    }
    return { node, x: startX + (index - sourceCount + 1) * nodeSpacing, y: startY + columnHeight / 2 };
  });
  const place = (id: FlowNodeId) => placed.find(({ node }) => node.id === id);
  const middleOf = (edge: FlowEdge): { x: number; y: number } | undefined => {
    const from = place(edge.from);
    const to = place(edge.to);
    return from && to
      ? { x: (from.x + nodeWidth + to.x) / 2, y: (from.y + to.y + nodeHeight) / 2 }
      : undefined;
  };
  const distance = (at: { x: number; y: number } | undefined, x: number, y: number) =>
    at ? Math.hypot(at.x - x, at.y - y) : Number.POSITIVE_INFINITY;
  const svgWidth = Math.max((laneCount + routeSteps + 2) * nodeSpacing + 40, 520);
  const svgHeight = 150 + columnHeight + routeRowCount * rowSpacing;

  const insert = (name: string, after: FlowNodeId | null) => {
    const added = addStep(form, after, name);
    if (added) {
      change(added.form);
      onSelect(added.id);
    }
  };
  const remove = (id: FlowNodeId) => {
    const step = stepIndexOf(id);
    const source = sourceIndexOf(id);
    const output = outputIndexOf(id);
    if (!form) return;
    change(
      routeNodeOf(id)
        ? removeRouteNode(form, id)
        : step !== undefined
        ? removeStep(form, step)
        : source !== undefined
          ? removeSource(form, source)
          : output !== undefined
            ? removeOutput(form, output)
            : setComputeKind(form, null),
    );
    onSelect(null);
  };

  return (
    <div
      className="flex flex-col gap-3"
      onKeyDown={(e) => {
        const target = e.target as HTMLElement;
        if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "z") return;
        if (target.closest("input, textarea, select, [contenteditable='true']")) return;
        e.preventDefault();
        if (e.shiftKey) redo();
        else undo();
      }}
    >
      {/* Palette */}
      <div
        role="toolbar"
        aria-label={t("pipelines.flow.palette", { defaultValue: "Compute palette" })}
        className="flex flex-wrap items-center gap-2"
      >
        <span className="text-caption font-semibold text-fg">
          {t("pipelines.flow.palette", { defaultValue: "Palette" })}:
        </span>
        {COMPUTE_KINDS.map((kind) => (
          <Button
            key={kind}
            size="sm"
            variant="secondary"
            data-testid={`palette-${kind}`}
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData("text/plain", kind);
            }}
            onClick={() => {
              change(setComputeKind(form, kind));
              onSelect("compute");
            }}
          >
            + {kind}
          </Button>
        ))}
        <Button
          size="sm"
          variant="secondary"
          data-testid="palette-source"
          onClick={() => {
            const added = addSource(form);
            change(added.form);
            onSelect(added.id);
          }}
        >
          {t("pipelines.flow.addSource")}
        </Button>
        <Button
          size="sm"
          variant="secondary"
          data-testid="palette-output"
          onClick={() => {
            const added = addOutput(form);
            change(added.form);
            onSelect(added.id);
          }}
        >
          {t("pipelines.flow.addOutput")}
        </Button>
        <Button
          size="sm"
          variant="secondary"
          data-testid="palette-route"
          onClick={() => {
            const added = addRoute(form);
            change(added.form);
            onSelect(added.id);
          }}
        >
          {t("pipelines.flow.addRoute")}
        </Button>
        {onTap ? (
          <Button
            size="sm"
            variant="secondary"
            data-testid="palette-debug"
            draggable
            disabledReason={
              selected === null || isSource(selected) ? t("pipelines.debug.pickWire") : undefined
            }
            disabled={selected === null || isSource(selected)}
            onDragStart={(e) => {
              e.dataTransfer.setData("text/plain", "debug");
            }}
            onClick={() => {
              const into = edges.find((edge) => edge.to === selected);
              if (into) onTap(wireKey(into));
            }}
          >
            {t("pipelines.debug.add")}
          </Button>
        ) : null}
        {form?.compute?.kind ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              change(setComputeKind(form, null));
              if (selected === "compute") onSelect(null);
            }}
          >
            {t("pipelines.flow.remove", { defaultValue: "Remove compute" })}
          </Button>
        ) : null}
      </div>

      <div
        role="toolbar"
        aria-label={t("pipelines.flow.tools")}
        className="flex flex-wrap items-center gap-2"
      >
        <Button size="sm" variant="ghost" disabled={past.length === 0} disabledReason={t("pipelines.flow.nothingToUndo")} onClick={undo} aria-keyshortcuts="Control+Z">
          {t("pipelines.flow.undo")}
        </Button>
        <Button size="sm" variant="ghost" disabled={future.length === 0} disabledReason={t("pipelines.flow.nothingToRedo")} onClick={redo} aria-keyshortcuts="Control+Shift+Z">
          {t("pipelines.flow.redo")}
        </Button>
        <span className="mx-1 h-4 w-px bg-border" aria-hidden="true" />
        <Button
          size="sm"
          variant="ghost"
          aria-label={t("pipelines.flow.zoomOut")}
          disabled={view !== "canvas" || (zoom ?? 1) <= 0.5}
          disabledReason={t("pipelines.flow.zoomClosed")}
          onClick={() => setZoom(Math.max(0.5, (zoom ?? 1) - 0.25))}
        >
          −
        </Button>
        <span className="min-w-12 text-center font-mono text-caption text-fg-muted" aria-live="polite">
          {zoom === null ? t("pipelines.flow.fitted") : `${Math.round(zoom * 100)} %`}
        </span>
        <Button
          size="sm"
          variant="ghost"
          aria-label={t("pipelines.flow.zoomIn")}
          disabled={view !== "canvas" || (zoom ?? 1) >= 2}
          disabledReason={t("pipelines.flow.zoomClosed")}
          onClick={() => setZoom(Math.min(2, (zoom ?? 1) + 0.25))}
        >
          +
        </Button>
        <Button size="sm" variant="ghost" disabled={view !== "canvas" || zoom === null} disabledReason={t("pipelines.flow.zoomClosed")} onClick={() => setZoom(null)}>
          {t("pipelines.flow.fit")}
        </Button>
        <span className="mx-1 h-4 w-px bg-border" aria-hidden="true" />
        <Button size="sm" variant="ghost" aria-pressed={view === "list"} onClick={() => setView(view === "list" ? "canvas" : "list")}>
          {t("pipelines.flow.asList")}
        </Button>
      </div>
      <p role="status" className="min-h-5 text-caption text-fg-muted" data-testid="flow-said">
        {said ?? ""}
      </p>

      {/* SVG Canvas */}
      {showsLive ? (
        <p className="text-caption text-fg-muted" data-testid="flow-live">
          {t("pipelines.flow.live")}
        </p>
      ) : null}
      {view === "list" ? (
        <ol aria-label={t("pipelines.flow.listLabel")} className="flex flex-col gap-1" data-testid="flow-list">
          {nodes.map((node) => {
            const at = lane.indexOf(node.id);
            const nodePaint = paint[node.id] ?? { state: "idle" };
            return (
              <li
                key={node.id}
                className={`flex flex-wrap items-center gap-2 rounded-md border px-2 py-1 ${selected === node.id ? "border-primary-soft-fg" : "border-border"}`}
              >
                <span className="font-semibold text-fg">{nodeName(node)}</span>
                <span className="font-mono text-caption text-fg-muted">{node.kind}</span>
                {node.summary ? <span className="truncate text-caption text-fg-muted">{node.summary}</span> : null}
                <span className={`text-caption ${nodePaint.state === "error" ? "text-danger-fg" : missing[node.id] ? "font-semibold text-fg" : "text-fg-muted"}`}>
                  {nodePaint.error ?? (nodePaint.state === "idle" ? stateWords(node.id) : nodePaint.state)}
                </span>
                <span className="ml-auto flex gap-1">
                  <Button size="sm" variant="secondary" onClick={() => open(node.id)}>
                    {t("pipelines.flow.open")}
                    <span className="sr-only"> {nodeName(node)} {node.kind}</span>
                  </Button>
                  {at >= 0 ? (
                    <>
                      <Button size="sm" variant="ghost" disabled={at === 0} disabledReason={t("pipelines.flow.atTop")} onClick={() => shift(node.id, -1)}>
                        {t("pipelines.flow.moveUp")}
                        <span className="sr-only"> {nodeName(node)} {node.kind}</span>
                      </Button>
                      <Button size="sm" variant="ghost" disabled={at === lane.length - 1} disabledReason={t("pipelines.flow.atBottom")} onClick={() => shift(node.id, 1)}>
                        {t("pipelines.flow.moveDown")}
                        <span className="sr-only"> {nodeName(node)} {node.kind}</span>
                      </Button>
                    </>
                  ) : null}
                  {node.id !== "source" && node.id !== "output" ? (
                    <Button size="sm" variant="ghost" onClick={() => remove(node.id)}>
                      {t("pipelines.flow.removeNode")}
                      <span className="sr-only"> {nodeName(node)} {node.kind}</span>
                    </Button>
                  ) : null}
                </span>
              </li>
            );
          })}
        </ol>
      ) : null}
      <div
        hidden={view !== "canvas"}
        className="w-full overflow-auto rounded-md border border-border bg-surface-subtle p-2"
      >
        {/*
          A group, not an image: the canvas holds the pipeline's nodes, and every node is a
          control a person tabs to and presses. `role="img"` promised a picture with nothing
          inside it, which is `nested-interactive` — a serious axe violation, and a screen
          reader that announces the diagram and then goes silent about the nine buttons in it.
        */}
        <svg
          role="group"
          data-testid="flow-canvas"
          aria-label={t("pipelines.flow.canvas", { defaultValue: "Pipeline canvas" })}
          viewBox={`0 0 ${svgWidth} ${svgHeight}`}
          className={zoom === null ? "h-40 min-w-full" : undefined}
          width={zoom === null ? undefined : svgWidth * zoom}
          height={zoom === null ? undefined : svgHeight * zoom}
          onPointerMove={(e) => {
            if (!wiring) return;
            const box = e.currentTarget.getBoundingClientRect();
            if (box.width <= 0 || box.height <= 0) return;
            setWiring({
              ...wiring,
              x: ((e.clientX - box.left) / box.width) * svgWidth,
              y: ((e.clientY - box.top) / box.height) * svgHeight,
            });
          }}
          onPointerUp={() => setWiring(null)}
          onPointerLeave={() => setWiring(null)}
          onDragOver={(e) => {
            e.preventDefault();
            e.dataTransfer.dropEffect = "copy";
          }}
          onDrop={(e) => {
            e.preventDefault();
            const kind = e.dataTransfer.getData("text/plain");
            if (kind === "debug" && onTap) {
              const box = e.currentTarget.getBoundingClientRect();
              const x = box.width > 0 ? ((e.clientX - box.left) / box.width) * svgWidth : -1;
              const y = box.height > 0 ? ((e.clientY - box.top) / box.height) * svgHeight : -1;
              const nearest =
                x < 0
                  ? edges.find((edge) => edge.to === selected)
                  : edges
                      .map((edge) => ({ edge, at: middleOf(edge) }))
                      .filter((one) => one.at !== undefined)
                      .sort((a, b) => distance(a.at, x, y) - distance(b.at, x, y))[0]?.edge;
              if (nearest) onTap(wireKey(nearest));
            } else if (kind.startsWith("processor:")) {
              // The node the drop landed on or behind. A canvas with no layout yet (no width, as
              // in a test) has no place to read, and the selection says where instead.
              const box = e.currentTarget.getBoundingClientRect();
              const x = box.width > 0 ? ((e.clientX - box.left) / box.width) * svgWidth : -1;
              const y = box.height > 0 ? ((e.clientY - box.top) / box.height) * svgHeight : -1;
              const under = placed.find(
                (spot) => x >= spot.x && x < spot.x + nodeSpacing && y >= spot.y - rowSpacing / 2,
              );
              insert(kind.slice(10), x < 0 ? selected : (under?.node.id ?? "source"));
            } else if (COMPUTE_KINDS.includes(kind as (typeof COMPUTE_KINDS)[number])) {
              change(setComputeKind(form, kind));
              onSelect("compute");
            }
          }}
        >
          <defs>
            <marker
              id="flow-arrow"
              viewBox="0 0 10 10"
              refX="8"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto-start-reverse"
            >
              <path d="M 0 1.5 L 8 5 L 0 8.5 z" className="fill-border-strong" />
            </marker>
          </defs>

          {/* Edges */}
          {edges.map((edge, edgeIndex) => {
            const from = place(edge.from);
            const to = place(edge.to);
            if (!from || !to) return null;
            const x1 = from.x + nodeWidth;
            const y1 = from.y + nodeHeight / 2;
            const x2 = to.x;
            const y2 = to.y + nodeHeight / 2;
            const key = wireKey(edge);
            const count = trace && !showsLive ? wireCount(trace, nodes, edge) : undefined;
            const tapped = taps.includes(key);
            // A wire into a node that failed in the last five minutes is red (PL-68).
            const failing = showsLive && paint[edge.to]?.state === "error";
            const mx = (x1 + x2) / 2;
            const my = (y1 + y2) / 2;
            return (
              <g key={`${edge.from}-${edge.to}`}>
                <line
                  x1={x1}
                  y1={y1}
                  x2={x2 - 4}
                  y2={y2}
                  className={tapped ? "stroke-primary-soft-fg" : failing ? "stroke-danger" : "stroke-border-strong"}
                  data-failing={failing ? "true" : undefined}
                  data-testid={`flow-edge-${key}`}
                  strokeWidth={2}
                  markerEnd="url(#flow-arrow)"
                />
                {/* A wider, invisible line takes the click: a wire opens the node it enters, whose
                    drawer shows what crossed it (T-3222). The node itself is the keyboard path. */}
                <line
                  x1={x1}
                  y1={y1}
                  x2={x2}
                  y2={y2}
                  aria-hidden="true"
                  data-testid={`flow-wire-${key}`}
                  className="cursor-pointer stroke-transparent"
                  strokeWidth={14}
                  onClick={() => open(edge.to)}
                />
                {/* The condition rides on the wire into its route, once (PL-73). */}
                {edge.route !== undefined && edges.findIndex((one) => one.route === edge.route) === edgeIndex ? (
                  <text
                    x={mx}
                    y={my + 16}
                    textAnchor="middle"
                    data-testid={`flow-route-when-${edge.route}`}
                    className="cursor-pointer select-none fill-fg font-mono text-caption"
                    onClick={() => open(`route-${edge.route ?? 0}-output-0`)}
                  >
                    {(() => {
                      const when = form?.routes?.[edge.route]?.when?.trim() ?? "";
                      if (!when) return t("pipelines.flow.routeNoCondition");
                      return when.length > 28 ? `${when.slice(0, 27)}…` : when;
                    })()}
                  </text>
                ) : null}
                {count !== undefined ? (
                  <text
                    x={mx}
                    y={my - 6}
                    textAnchor="middle"
                    data-testid={`flow-wire-count-${key}`}
                    className="pointer-events-none select-none fill-fg font-mono text-caption"
                  >
                    {count}
                  </text>
                ) : null}
                {queue && edge.to === "output" ? (
                  // The queue NiFi shows on a connection: what waits before the output, live.
                  <text
                    x={mx}
                    y={my - 20}
                    textAnchor="middle"
                    data-testid="flow-queue"
                    data-high={queue.high ? "true" : undefined}
                    className={`pointer-events-none select-none font-mono text-caption ${queue.high ? "fill-warning" : "fill-fg-muted"}`}
                  >
                    <title>{t(queue.high ? "pipelines.flow.queueHigh" : "pipelines.flow.queue", { count: queue.depth, limit: QUEUE_WARN })}</title>
                    {t("pipelines.flow.queueShort", { count: queue.depth })}
                  </text>
                ) : null}
                {tapped ? (
                  <g
                    role="button"
                    tabIndex={0}
                    aria-label={t("pipelines.debug.remove")}
                    data-testid={`flow-tap-${key}`}
                    className="focus-ring cursor-pointer"
                    onClick={() => onTap?.(key)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " " || e.key === "Delete") {
                        e.preventDefault();
                        onTap?.(key);
                      }
                    }}
                  >
                    <rect x={mx - 22} y={my + 4} width={44} height={16} rx={8} className="fill-primary-soft stroke-primary-soft-fg" />
                    <text x={mx} y={my + 16} textAnchor="middle" className="select-none fill-primary-soft-fg text-caption">
                      {t("pipelines.debug.badge")}
                    </text>
                  </g>
                ) : null}
              </g>
            );
          })}

          {/* Nodes */}
          {placed.map(({ node, x, y }, idx) => {
            const isSelected = selected === node.id;
            const nodePaint = paint[node.id] ?? { state: "idle" };
            // The canvas paints with the same tokens as the page around it (UI-30): an SVG
            // attribute takes a colour value, which is why these were six literals and why the
            // whole diagram stayed light grey on white in the dark theme. A class is a token.
            const strokeClass =
              nodePaint.state === "error"
                ? "stroke-danger"
                : nodePaint.state === "skipped"
                  ? "stroke-fg-subtle"
                  : nodePaint.state === "ok"
                    ? "stroke-success"
                    : isSelected
                      ? "stroke-primary-soft-fg"
                      : "stroke-border";

            const summary =
              node.summary.length > 40 ? `${node.summary.slice(0, 39)}…` : node.summary;
            const errorSummary =
              nodePaint.error && nodePaint.error.length > 60
                ? `${nodePaint.error.slice(0, 59)}…`
                : nodePaint.error;

            return (
              <g
                key={node.id}
                role="button"
                tabIndex={0}
                data-testid={`flow-node-${node.id}`}
                data-state={nodePaint.state}
                aria-pressed={isSelected}
                aria-label={`${node.label}: ${node.kind}, ${nodePaint.state === "idle" ? stateWords(node.id) : nodePaint.state}`}
                className="focus-ring cursor-pointer"
                onClick={() => onSelect(node.id)}
                onDoubleClick={() => open(node.id)}
                // A wire let go on a node ends there, as on its input port: a source has no
                // port, and `connect` says why it takes no wire instead of the canvas
                // dropping it without a word (T-4271).
                onPointerUp={(e) => {
                  if (!wiring) return;
                  e.stopPropagation();
                  wire(wiring.from, node.id);
                  setWiring(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    open(node.id);
                  } else if (e.key === " ") {
                    e.preventDefault();
                    onSelect(node.id);
                  } else if (
                    // The first source and the output are what the pipeline is; the source
                    // section clears the first one, and a pipeline always writes somewhere.
                    (e.key === "Delete" || e.key === "Backspace") &&
                    node.id !== "source" &&
                    node.id !== "output"
                  ) {
                    e.preventDefault();
                    remove(node.id);
                  } else if (e.key === "ArrowRight") {
                    e.preventDefault();
                    const nextIdx = Math.min(idx + 1, nodes.length - 1);
                    onSelect(nodes[nextIdx].id);
                  } else if (e.key === "ArrowLeft") {
                    e.preventDefault();
                    const prevIdx = Math.max(idx - 1, 0);
                    onSelect(nodes[prevIdx].id);
                  }
                }}
              >
                <rect
                  x={x}
                  y={y}
                  width={nodeWidth}
                  height={nodeHeight}
                  rx={8}
                  className={`fill-surface ${strokeClass}`}
                  strokeWidth={isSelected ? 2.5 : 1.5}
                />
                <text
                  x={x + 12}
                  y={y + 22}
                  className="select-none fill-fg text-body font-semibold"
                >
                  {nodeName(node)}
                </text>
                <text
                  x={x + nodeWidth - 12}
                  y={y + 22}
                  textAnchor="end"
                  className="select-none fill-fg-muted font-mono text-caption"
                >
                  {node.kind}
                </text>
                {summary ? (
                  <text
                    x={x + 12}
                    y={y + 44}
                    className="select-none fill-fg-muted text-caption"
                  >
                    {summary}
                  </text>
                ) : null}

                {nodePaint.eventsIn !== undefined ? (
                  <text
                    x={x + 12}
                    y={y + 66}
                    className="select-none fill-fg-muted font-mono text-caption"
                  >
                    ↓ {nodePaint.eventsIn}
                    {showsLive ? t("pipelines.flow.perMinute") : ""}  ↑ {nodePaint.eventsOut ?? 0}
                    {showsLive ? t("pipelines.flow.perMinute") : ""}
                  </text>
                ) : null}

                {errorSummary ? (
                  <text
                    x={x + 12}
                    y={y + 86}
                    className="select-none fill-danger font-mono text-caption"
                  >
                    {errorSummary}
                  </text>
                ) : nodePaint.state === "ok" ? (
                  <text
                    x={x + 12}
                    y={y + 86}
                    className="select-none fill-success text-caption font-medium"
                  >
                    ok
                  </text>
                ) : nodePaint.state === "skipped" ? (
                  <text
                    x={x + 12}
                    y={y + 86}
                    className="select-none fill-fg-subtle text-caption font-medium"
                  >
                    skipped
                  </text>
                ) : (
                  <text
                    x={x + 12}
                    y={y + 86}
                    data-testid={`flow-node-state-${node.id}`}
                    className={`select-none text-caption font-medium ${missing[node.id] ? "fill-fg font-semibold" : "fill-fg-muted"}`}
                  >
                    {stateWords(node.id)}
                  </text>
                )}
              </g>
            );
          })}

          {/* Ports: drag from a node's output port (right) to another's input port (left). The
              wire only sets the order (PL-56); the list view below does the same by keyboard. */}
          {placed.map(({ node, x, y }) => (
            <g key={`ports-${node.id}`} aria-hidden="true">
              {isSource(node.id) ? null : (
                <circle
                  cx={x}
                  cy={y + nodeHeight / 2}
                  r={7}
                  data-testid={`flow-port-in-${node.id}`}
                  className={`fill-surface stroke-2 ${wiring ? "stroke-primary-soft-fg" : "stroke-border-strong"}`}
                  onPointerUp={(e) => {
                    e.stopPropagation();
                    if (wiring) wire(wiring.from, node.id);
                    setWiring(null);
                  }}
                />
              )}
              {isOutput(node.id) ? null : (
                <circle
                  cx={x + nodeWidth}
                  cy={y + nodeHeight / 2}
                  r={7}
                  data-testid={`flow-port-out-${node.id}`}
                  className="cursor-crosshair fill-surface stroke-border-strong stroke-2"
                  onPointerDown={(e) => {
                    e.stopPropagation();
                    setSaid(null);
                    setWiring({ from: node.id, x: x + nodeWidth, y: y + nodeHeight / 2 });
                  }}
                />
              )}
            </g>
          ))}
          {wiring
            ? (() => {
                const from = place(wiring.from);
                return from ? (
                  <line
                    x1={from.x + nodeWidth}
                    y1={from.y + nodeHeight / 2}
                    x2={wiring.x}
                    y2={wiring.y}
                    className="pointer-events-none stroke-primary-soft-fg"
                    strokeWidth={2}
                    strokeDasharray="6 4"
                  />
                ) : null;
              })()
            : null}
        </svg>
      </div>

      {/* Below the canvas, not above it: with the 75 processors in between, a dialog at
          1280x720 could not show a palette and the canvas it drops onto at the same time, and a
          drag that scrolled the canvas into view left its palette behind. Both palettes touch
          the canvas now. */}
      {/* One fold for all of them, shut: most pipelines take none, and nine open groups were
          the tallest thing on the form (T-2754). */}
      <Field id="flow-palette-search" label={t("pipelines.flow.search")}>
        <Input
          id="flow-palette-search"
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("pipelines.flow.searchPlaceholder")}
        />
      </Field>
      {needle ? (
        found.length === 0 ? (
          <p className="text-caption text-fg-muted">{t("pipelines.flow.searchNone", { query: query.trim() })}</p>
        ) : (
          <ul className="flex flex-col gap-1" aria-label={t("pipelines.flow.searchFound", { count: found.length })}>
            {found.map((processor) => (
              <li key={processor.name} className="flex items-baseline gap-2">
                <Button
                  size="sm"
                  variant="secondary"
                  className="shrink-0 font-mono"
                  data-testid={`palette-found-${processor.name}`}
                  draggable
                  onDragStart={(e) => {
                    e.dataTransfer.setData("text/plain", `processor:${processor.name}`);
                  }}
                  onClick={() => insert(processor.name, selected)}
                >
                  + {processor.name}
                </Button>
                <span className="text-caption text-fg-muted">
                  {processor.category} · {plainSummary(processor.summary)}
                </span>
              </li>
            ))}
          </ul>
        )
      ) : null}
      <details className="flex flex-col gap-1" data-testid="palette-processors" hidden={needle !== ""}>
        <summary
          id="flow-processors"
          className="focus-ring cursor-pointer rounded-md text-caption font-semibold text-fg"
        >
          {t("pipelines.flow.processors")} ({PROCESSOR_COUNT})
        </summary>
        <p className="text-caption text-fg-muted">{t("pipelines.flow.processorsHint")}</p>
        {PROCESSOR_GROUPS.map(({ category, processors }) => (
          <details key={category} className="rounded-md border border-border bg-surface">
            <summary className="focus-ring cursor-pointer rounded-md px-2 py-1 text-caption font-medium text-fg">
              {category} ({processors.length})
            </summary>
            <ul className="flex flex-col gap-1 p-2" aria-label={category}>
              {processors.map((processor) => (
                <li key={processor.name} className="flex items-baseline gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    className="shrink-0 font-mono"
                    data-testid={`palette-processor-${processor.name}`}
                    draggable
                    onDragStart={(e) => {
                      e.dataTransfer.setData("text/plain", `processor:${processor.name}`);
                    }}
                    onClick={() => insert(processor.name, selected)}
                  >
                    + {processor.name}
                  </Button>
                  <span className="text-caption text-fg-muted">
                    {plainSummary(processor.summary)}
                  </span>
                </li>
              ))}
            </ul>
          </details>
        ))}
      </details>
    </div>
  );
}

export interface StepBlockProps {
  entry: StepForm;
  onChange: (entry: StepForm) => void;
  onRemove: () => void;
  /** The pipeline's own runner variables, the `envVar` of each secretRef, offered on secret fields. */
  variables?: string[];
}

/**
 * The selected step's own Bento block (PL-56): a form generated from the runner's field tree of
 * that processor (T-3088), and the block as YAML, `<processor>: <config>`, nothing of a
 * neighbour's. Both edit the same config; what is typed reaches the form only while it is one
 * processor the runner ships, and until then the form keeps the last block that was, with the
 * reason at the field or under the text. Mount it with the node's id as `key`: the text is the
 * author's until another node is chosen.
 */
// ponytail: a textarea like the compute node's mapping, not Monaco; swap in MonacoSourceView
// when authors ask for completion inside a processor's fields.
export function StepBlock({ entry, onChange, onRemove, variables = [] }: StepBlockProps): JSX.Element {
  const { t } = useTranslation();
  const processor = processorOf(entry.step);
  const [text, setText] = useState(() =>
    stringifyYaml(processor ? { [processor[0]]: processor[1] } : entry.step),
  );
  const [problem, setProblem] = useState<string | null>(null);
  const [view, setView] = useState<"form" | "yaml">("form");
  // What the form starts from, taken when it opens: the form owns its text from then on, so a
  // YAML box half typed is not rewritten under the cursor by its own round trip.
  const [seed, setSeed] = useState<{ data: unknown } | null>(null);
  const [boxError, setBoxError] = useState<ErrorSchema | undefined>(undefined);
  const forms = useProcessorForms();
  const id = "flow-step-yaml";
  const found = (processorCatalogue as Processor[]).find(({ name }) => name === processor?.[0]);
  const form = processor ? forms?.processors[processor[0]] : undefined;
  const shown = form && view === "form" ? "form" : "yaml";
  if (processor && form && seed === null) {
    // The forms arrive after the first render the first time; the seed is taken then.
    setSeed({ data: toFormData(processor[1], form.schema, form.uiSchema) });
  }
  const uiSchema = useMemo(
    () =>
      form
        ? {
            ...withHelp(form.uiSchema, t("pipelines.flow.stepSecretHelp")),
            "ui:submitButtonOptions": { norender: true },
          }
        : undefined,
    [form, t],
  );

  const typed = (next: string) => {
    setText(next);
    let block: unknown;
    try {
      block = parseYaml(next);
    } catch (error) {
      setProblem(
        t("pipelines.flow.stepYamlInvalid", {
          reason: (error as Error).message.split("\n")[0],
        }),
      );
      return;
    }
    const names =
      block && typeof block === "object" && !Array.isArray(block) ? Object.keys(block) : [];
    if (names.length !== 1) {
      setProblem(t("pipelines.flow.stepYamlOne"));
    } else if (!PROCESSOR_NAMES.has(names[0])) {
      setProblem(t("pipelines.flow.stepYamlUnknown", { name: names[0] }));
    } else {
      setProblem(null);
      onChange({ ...entry, step: { ...entry.step, processor: block as Record<string, unknown> } });
    }
  };

  const filled = (data: unknown) => {
    if (!processor || !form) return;
    let config: unknown;
    try {
      config = fromFormData(data, form.schema, form.uiSchema, "", processor[1]);
    } catch (error) {
      if (!(error instanceof YamlFieldError)) throw error;
      setBoxError(
        errorAt(error.field, t("pipelines.flow.stepYamlInvalid", { reason: error.detail })),
      );
      return;
    }
    setBoxError(undefined);
    // The form reports itself once on opening; a block it did not change is not an edit.
    if (JSON.stringify(config ?? {}) === JSON.stringify(processor[1] ?? {})) return;
    const block = { [processor[0]]: config ?? {} };
    setText(stringifyYaml(block));
    setProblem(null);
    onChange({ ...entry, step: { ...entry.step, processor: block } });
  };

  const switchTo = (next: "form" | "yaml") => {
    if (next === "form" && processor && form) {
      setSeed({ data: toFormData(processor[1], form.schema, form.uiSchema) });
      setBoxError(undefined);
    }
    setView(next);
  };

  return (
    <div
      className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3"
      data-testid="flow-node-editor-step"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-caption font-semibold text-fg">
          {t("pipelines.flow.node.step")} ({processor?.[0] ?? String(entry.step.kind ?? "mapping")})
        </span>
        <Button size="sm" variant="ghost" data-testid="flow-step-remove" onClick={onRemove}>
          {t("pipelines.flow.removeStep")}
        </Button>
      </div>
      {processor && form ? (
        <Tabs
          id="flow-step-view"
          label={t("pipelines.flow.stepView")}
          variant="pill"
          tabs={[
            { value: "form", label: t("pipelines.flow.stepForm") },
            { value: "yaml", label: t("pipelines.flow.stepYamlTab") },
          ]}
          value={shown}
          onChange={switchTo}
        />
      ) : null}
      {processor && form && shown === "form" ? (
        <div
          {...tabPanelProps("flow-step-view", "form")}
          data-testid="flow-step-form"
          className="flex flex-col gap-2"
        >
          {found ? <p className="text-caption text-fg-muted">{plainSummary(found.summary)}</p> : null}
          <SchemaForm<unknown>
            // Keyed on the seed, so reopening the form after a YAML edit starts from that edit.
            key={JSON.stringify(seed?.data ?? null)}
            schema={withVariables(form.schema, form.uiSchema, variables)}
            uiSchema={uiSchema}
            formData={seed?.data}
            extraErrors={boxError}
            onSubmit={() => undefined}
            onChange={filled}
          />
        </div>
      ) : null}
      {processor && shown === "yaml" ? (
        <div {...(form ? tabPanelProps("flow-step-view", "yaml") : {})}>
          <Field
            id={id}
            label={t("pipelines.flow.stepYaml")}
            description={found ? plainSummary(found.summary) : undefined}
            help={t("pipelines.flow.stepYamlHint")}
            errors={problem ? [problem] : undefined}
          >
            {/* The Field wires `aria-describedby` and `aria-invalid` onto the control it wraps,
                so the textarea no longer rebuilds the ids by hand (T-2314). */}
            <Textarea
              id={id}
              data-testid="flow-step-yaml"
              rows={8}
              spellCheck={false}
              className="p-2 font-mono text-caption"
              value={text}
              onChange={(e) => typed(e.target.value)}
            />
          </Field>
        </div>
      ) : processor ? null : (
        <>
          <CodeBlock
            label={t("codeBlock.code")}
            data-testid="flow-step-readonly"
            className="max-h-48 rounded-md border border-border bg-surface-subtle p-2 font-mono text-caption break-words whitespace-pre-wrap text-fg"
          >
            {text}
          </CodeBlock>
          <p className="text-caption text-fg-muted">{t("pipelines.flow.stepReadOnly")}</p>
        </>
      )}
    </div>
  );
}

export interface SourceBlockProps {
  project: string;
  source: SourceForm;
  dataSources: Manifest[];
  endpoints: Manifest[];
  locale: string;
  onChange: (source: SourceForm) => void;
  /** Absent for the first source: a pipeline always reads one (T-3978). */
  onRemove?: () => void;
}

/**
 * One source (PL-52): one DataSource, or one Endpoint read through. A source after the first is
 * merged with the others through a `broker` input, so it needs nothing but what it reads; the
 * first source's node opens the same block (T-3978), its sample and access panel stay in the
 * studio's own section below.
 */
export function SourceBlock({
  project,
  source,
  dataSources,
  endpoints,
  locale,
  onChange,
  onRemove,
}: SourceBlockProps): JSX.Element {
  const { t } = useTranslation();
  const id = "flow-source-ref";
  const value = source.dataSourceRef
    ? `DataSource:${source.dataSourceRef}`
    : source.endpointRef
      ? `Endpoint:${source.endpointRef}`
      : "";

  return (
    <div
      className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3"
      data-testid="flow-node-editor-source"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-caption font-semibold text-fg">{t("pipelines.flow.node.source")}</span>
        {onRemove ? (
          <Button size="sm" variant="ghost" data-testid="flow-source-remove" onClick={onRemove}>
            {t("pipelines.flow.removeSource")}
          </Button>
        ) : null}
      </div>
      <Field
        id={id}
        label={t("pipelines.flow.sourceReads")}
        help={t("pipelines.flow.sourceReadsHint")}
      >
        <Select
          id={id}
          data-testid="flow-source-ref"
          value={value}
          onChange={(event) => {
            const [kind, ...name] = event.target.value.split(":");
            const picked = name.join(":");
            onChange({
              ...source,
              dataSourceRef: kind === "DataSource" ? picked : undefined,
              endpointRef: kind === "Endpoint" ? picked : undefined,
              // A query is what an endpoint is asked; a DataSource reads its own connection.
              query: kind === "Endpoint" ? source.query : undefined,
            });
          }}
        >
          <option value="">—</option>
          <optgroup label={t("nav.datasources")}>
            {dataSources.map((manifest) => (
              <option key={manifest.metadata.name} value={`DataSource:${manifest.metadata.name}`}>
                {localized(manifest.metadata.title, locale, manifest.metadata.name)}
              </option>
            ))}
          </optgroup>
          <optgroup label={t("nav.endpoints")}>
            {endpoints.map((manifest) => (
              <option key={manifest.metadata.name} value={`Endpoint:${manifest.metadata.name}`}>
                {localized(manifest.metadata.title, locale, manifest.metadata.name)}
              </option>
            ))}
          </optgroup>
        </Select>
      </Field>
      {source.endpointRef ? (
        // What an endpoint source reads (PL-42): the type its space models, the attributes and
        // a filter, as the first source asks them; without a type the runner reads nothing (T-3088).
        <EndpointQuery
          project={project}
          space={(() => {
            const ep = endpoints.find((e) => e.metadata.name === source.endpointRef);
            return ep ? spaceOf(ep) : undefined;
          })()}
          query={source.query ?? {}}
          onChange={(query) => onChange({ ...source, query })}
        />
      ) : null}
    </div>
  );
}

/** The type, attributes and filter of an endpoint source; empty values leave the query. */
function EndpointQuery({
  project,
  space,
  query,
  onChange,
}: {
  project: string;
  space: string | undefined;
  query: Record<string, unknown>;
  onChange: (query: Record<string, unknown>) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const set = (key: string, value: unknown) => {
    const next = { ...query, [key]: value };
    if (value === undefined || value === "" || (Array.isArray(value) && value.length === 0)) {
      delete next[key];
    }
    onChange(next);
  };
  const attrs = Array.isArray(query.attrs) ? (query.attrs as string[]) : [];
  return (
    <div className="grid gap-2 sm:grid-cols-2" data-testid="flow-source-query">
      <Field id="flow-source-type" label={t("pipelines.field.queryType")} required>
        <TypePicker
          id="flow-source-type"
          label={t("pipelines.field.queryType")}
          labelled
          project={project}
          space={space}
          value={typeof query.type === "string" && query.type ? [query.type] : []}
          onChange={(types) => set("type", types[0])}
        />
      </Field>
      <Field id="flow-source-attrs" label={t("pipelines.field.attrs")} help={t("pipelines.flow.attrsHint")}>
        <AttributePicker
          id="flow-source-attrs"
          label={t("pipelines.field.attrs")}
          labelled
          multiple
          project={project}
          space={space}
          type={typeof query.type === "string" && query.type ? query.type : undefined}
          value={attrs}
          onChange={(names) => set("attrs", names)}
        />
      </Field>
      <Field id="flow-source-q" label={t("pipelines.field.q")} help={t("pipelines.studio.qHint")}>
        <Input
          id="flow-source-q"
          value={typeof query.q === "string" ? query.q : ""}
          onChange={(event) => set("q", event.target.value.trim() ? event.target.value : undefined)}
        />
      </Field>
    </div>
  );
}

export interface OutputBlockProps {
  project: string;
  output: OutputForm;
  /** The Endpoints this pipeline may write through, each with its URN when it has one. */
  targets: { manifest: Manifest; urn: string | undefined }[];
  locale: string;
  onChange: (output: OutputForm) => void;
  /** Absent for the first output: a pipeline always writes through one (T-3978). */
  onRemove?: () => void;
}

/**
 * One output (PL-55), the first one's node included (T-3978): the Endpoint it writes through,
 * the type and the write mode. The runner fans every message out to all outputs, so each is a
 * target the pipeline writes as well, not a route a message takes.
 */
export function OutputBlock({ project, output, targets, locale, onChange, onRemove }: OutputBlockProps): JSX.Element {
  const { t } = useTranslation();
  const target = targets.find((candidate) => candidate.urn === output.targetEndpoint);
  return (
    <div
      className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3"
      data-testid="flow-node-editor-output"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-caption font-semibold text-fg">{t("pipelines.flow.node.output")}</span>
        {onRemove ? (
          <Button size="sm" variant="ghost" data-testid="flow-output-remove" onClick={onRemove}>
            {t("pipelines.flow.removeOutput")}
          </Button>
        ) : null}
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        <Field id="flow-output-target" label={t("pipelines.flow.outputWrites")} help={t("pipelines.flow.outputWritesHint")} required>
          <Select
            id="flow-output-target"
            data-testid="flow-output-target"
            value={output.targetEndpoint ?? ""}
            onChange={(event) => onChange({ ...output, targetEndpoint: event.target.value || undefined })}
          >
            <option value="">—</option>
            {/* A URN the list no longer offers stays visible rather than silently replaced. */}
            {output.targetEndpoint && !target ? (
              <option value={output.targetEndpoint}>{output.targetEndpoint}</option>
            ) : null}
            {targets
              .filter((candidate) => candidate.urn)
              .map(({ manifest, urn }) => (
                <option key={manifest.metadata.name} value={urn}>
                  {localized(manifest.metadata.title, locale, manifest.metadata.name)}
                </option>
              ))}
          </Select>
        </Field>
        <Field id="flow-output-mode" label={t("pipelines.field.outputMode")}>
          <Select
            id="flow-output-mode"
            value={output.mode ?? "upsert"}
            onChange={(event) => onChange({ ...output, mode: event.target.value })}
          >
            {OUTPUT_MODES.map((mode) => (
              <option key={mode} value={mode}>
                {t(`pipelines.field.modeChoice.${mode}`)}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="flow-output-type" label={t("pipelines.field.outputType")}>
          <TypePicker
            id="flow-output-type"
            label={t("pipelines.field.outputType")}
            labelled
            project={project}
            space={target ? spaceOf(target.manifest) : undefined}
            value={output.type ? [output.type] : []}
            onChange={(types) => onChange({ ...output, type: types[0] })}
          />
        </Field>
      </div>
    </div>
  );
}

export interface RouteBlockProps {
  project: string;
  route: RouteForm;
  /** The output of the route the selected node is, when it is one. */
  output: number | undefined;
  targets: OutputBlockProps["targets"];
  locale: string;
  onChange: (route: RouteForm) => void;
  onRemove: () => void;
}

/**
 * A route (PL-73): its name, the condition its records meet, and the selected output of the
 * route. A record takes the first route whose condition holds; the rest go to the lane's outputs.
 */
export function RouteBlock({ project, route, output, targets, locale, onChange, onRemove }: RouteBlockProps): JSX.Element {
  const { t } = useTranslation();
  const outputs = route.outputs ?? [];
  const selected = output === undefined ? undefined : outputs[output];
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3" data-testid="flow-node-editor-route">
      <div className="flex items-center justify-between gap-2">
        <span className="text-caption font-semibold text-fg">{t("pipelines.flow.node.routeOutput", { name: route.name ?? "" })}</span>
        <Button size="sm" variant="ghost" data-testid="flow-route-remove" onClick={onRemove}>
          {t("pipelines.flow.removeRoute")}
        </Button>
      </div>
      <Field id="flow-route-name" label={t("pipelines.flow.routeName")} help={t("pipelines.flow.routeNameHint")} required>
        <Input
          id="flow-route-name"
          data-testid="flow-route-name"
          value={route.name ?? ""}
          onChange={(event) => onChange({ ...route, name: event.target.value })}
        />
      </Field>
      <Field id="flow-route-when" label={t("pipelines.flow.routeWhen")} help={t("pipelines.flow.routeWhenHint")} required>
        <Textarea
          id="flow-route-when"
          data-testid="flow-route-when"
          className="font-mono"
          rows={2}
          value={route.when ?? ""}
          onChange={(event) => onChange({ ...route, when: event.target.value })}
        />
      </Field>
      {selected && output !== undefined ? (
        <OutputBlock
          key={output}
          project={project}
          output={selected}
          targets={targets}
          locale={locale}
          onChange={(changed) => onChange({ ...route, outputs: outputs.map((was, at) => (at === output ? changed : was)) })}
          onRemove={() =>
            outputs.length <= 1 ? onRemove() : onChange({ ...route, outputs: outputs.filter((_, at) => at !== output) })
          }
        />
      ) : null}
      <Button
        size="sm"
        variant="secondary"
        className="self-start"
        data-testid="flow-route-add-output"
        onClick={() => onChange({ ...route, outputs: [...outputs, { mode: "upsert" }] })}
      >
        {t("pipelines.flow.routeAddOutput")}
      </Button>
    </div>
  );
}
