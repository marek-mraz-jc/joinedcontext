/**
 * The graph edits of the pipeline studio, as pure functions over the form (PL-56, PL-68, T-3221).
 *
 * The manifest stores no edges: the steps run in list order, every source feeds the head of the
 * lane and its tail feeds every output. A wire a person draws therefore only ever sets that
 * order; a wire the runner cannot run is refused with the reason instead of being kept.
 */
import type { PipelineForm, StepForm } from "./PipelineEditor";
import { outputIndexOf, routeNodeOf, sourceIndexOf, stepIndexOf, toFlow } from "./PipelineFlow";
import type { FlowNodeId } from "./PipelineFlow";

const isSource = (id: FlowNodeId): boolean => id === "source" || sourceIndexOf(id) !== undefined;
const isOutput = (id: FlowNodeId): boolean =>
  id === "output" || outputIndexOf(id) !== undefined || routeNodeOf(id)?.output !== undefined;

/** Why a wire was refused; each is a sentence under `pipelines.flow.wire.<reason>`. */
export type WireRefusal = "intoSource" | "outOfOutput" | "self" | "middleIntoOutput" | "route";

export type WireResult =
  | { form: PipelineForm; id: FlowNodeId }
  | { unchanged: true }
  | { refused: WireRefusal };

/** The lane's nodes in the order they run: the steps before the compute step, it, the rest. */
export function laneOf(form: PipelineForm | undefined): FlowNodeId[] {
  return toFlow(form)
    .nodes.map((node) => node.id)
    .filter((id) => !isSource(id) && !isOutput(id) && routeNodeOf(id) === undefined);
}

/**
 * The form with its lane in `order`, and where each node went: a step is named by its index in
 * `processors`, so moving one renames it. A step behind the compute step carries `after: true`;
 * one before it carries no `after` at all, as the form writes it.
 */
function withLane(
  form: PipelineForm,
  order: FlowNodeId[],
): { form: PipelineForm; renamed: Map<FlowNodeId, FlowNodeId> } {
  const steps = form.processors ?? [];
  const renamed = new Map<FlowNodeId, FlowNodeId>();
  const processors: StepForm[] = [];
  let behindCompute = false;
  for (const id of order) {
    if (id === "compute") {
      behindCompute = true;
      renamed.set(id, id);
      continue;
    }
    const entry = steps[stepIndexOf(id) ?? -1];
    if (!entry) continue;
    renamed.set(id, `step-${processors.length}`);
    processors.push(behindCompute ? { step: entry.step, after: true } : { step: entry.step });
  }
  return { form: { ...form, processors }, renamed };
}

/**
 * A wire from `from`'s output port to `to`'s input port (PL-56): into a step it moves that step
 * behind `from` (the head of the lane when `from` is a source); into an output it is already there
 * when `from` is the lane's tail, and refused from anywhere else, because the runner writes only
 * what the last step gives. Nothing goes into a source and nothing comes out of an output.
 */
export function connect(form: PipelineForm | undefined, from: FlowNodeId, to: FlowNodeId): WireResult {
  if (from === to) return { refused: "self" };
  if (isSource(to)) return { refused: "intoSource" };
  if (isOutput(from)) return { refused: "outOfOutput" };
  // A route's nodes hang off the lane's tail in their own order (PL-73): the wires the canvas
  // draws are there already, and any other is refused rather than moving a step across routes.
  if (routeNodeOf(from) || routeNodeOf(to)) {
    const drawn = toFlow(form).edges.some((edge) => edge.from === from && edge.to === to);
    return drawn ? { unchanged: true } : { refused: "route" };
  }
  const lane = laneOf(form);
  if (isOutput(to)) {
    const tail = lane[lane.length - 1];
    return (tail === undefined ? isSource(from) : from === tail) ? { unchanged: true } : { refused: "middleIntoOutput" };
  }
  if (!form) return { unchanged: true };
  const rest = lane.filter((id) => id !== to);
  const at = isSource(from) ? 0 : rest.indexOf(from) + 1;
  if (lane.indexOf(to) === at && (at === 0 || lane[at - 1] === from)) return { unchanged: true };
  const order = [...rest.slice(0, at), to, ...rest.slice(at)];
  const moved = withLane(form, order);
  return { form: moved.form, id: moved.renamed.get(to) ?? to };
}

/** The form with lane node `id` one place earlier (`-1`) or later (`1`); nothing past an end. */
export function move(
  form: PipelineForm | undefined,
  id: FlowNodeId,
  by: -1 | 1,
): { form: PipelineForm; id: FlowNodeId } | undefined {
  if (!form) return undefined;
  const lane = laneOf(form);
  const at = lane.indexOf(id);
  const to = at + by;
  if (at < 0 || to < 0 || to >= lane.length) return undefined;
  const order = [...lane];
  [order[at], order[to]] = [order[to], order[at]];
  const moved = withLane(form, order);
  return { form: moved.form, id: moved.renamed.get(id) ?? id };
}

/**
 * What a node lacks before it can run, said before any test (PL-68): a source with nothing to
 * read, an output with no Endpoint, a compute step or a mapping with nothing in it.
 */
export function missingOf(form: PipelineForm | undefined, id: FlowNodeId): string | undefined {
  const named = (value: unknown) => typeof value === "string" && value.trim() !== "";
  const reads = (source: { dataSourceRef?: unknown; endpointRef?: unknown } | undefined) =>
    named(source?.dataSourceRef) ||
    named(source?.endpointRef) ||
    (typeof source?.endpointRef === "object" && source.endpointRef !== null && named((source.endpointRef as { name?: unknown }).name));
  if (id === "source") return reads(form?.source) ? undefined : "source";
  const inRoute = routeNodeOf(id);
  if (inRoute) {
    const route = form?.routes?.[inRoute.route];
    if (!named(route?.when)) return "condition";
    if (inRoute.output !== undefined) return named(route?.outputs?.[inRoute.output]?.targetEndpoint) ? undefined : "output";
    const step = route?.steps?.[inRoute.step ?? -1];
    const processor = step?.processor;
    const [name, config] =
      processor && typeof processor === "object"
        ? (Object.entries(processor)[0] ?? [])
        : [step?.kind === "bloblang" ? "mapping" : "", step?.bloblang];
    return ["mapping", "mutation", "bloblang"].includes(name ?? "") && !named(config) ? "mapping" : undefined;
  }
  const source = sourceIndexOf(id);
  if (source !== undefined) return reads(form?.moreSources?.[source]) ? undefined : "source";
  if (id === "output") return named(form?.targetEndpoint) ? undefined : "output";
  const output = outputIndexOf(id);
  if (output !== undefined) return named(form?.moreOutputs?.[output]?.targetEndpoint) ? undefined : "output";
  if (id === "compute") {
    const compute = form?.compute;
    if (compute?.kind === "bloblang") return named(compute.bloblang) ? undefined : "mapping";
    if (compute?.kind === "mapping") return named(compute.mappingRef) ? undefined : "mappingRef";
    if (compute?.kind === "wasm" || compute?.kind === "container") return named(compute.module) ? undefined : "module";
    return undefined;
  }
  const step = form?.processors?.[stepIndexOf(id) ?? -1]?.step.processor;
  if (step && typeof step === "object") {
    const [name, config] = Object.entries(step)[0] ?? [];
    if (["mapping", "mutation", "bloblang"].includes(name ?? "") && !named(config)) return "mapping";
  }
  return undefined;
}
