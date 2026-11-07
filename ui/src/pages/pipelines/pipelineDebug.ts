/**
 * Debugging a pipeline on its graph (T-3222, PL-43, PL-67): what crossed each wire, what a node
 * changed in a message, one message at a time, and one step rerun on an input a person pinned.
 * Everything here reads the dry-run trace; nothing is written to a space.
 */
import type { PipelineForm } from "./PipelineEditor";
import { isLane, outputIndexOf, routeNodeOf, stepIndexOf } from "./PipelineFlow";
import type { FlowEdge, FlowNode, FlowNodeId } from "./PipelineFlow";
import type { SampleFormat, Trace } from "../../api/pipelineTest";

const isOutput = (id: FlowNodeId): boolean =>
  id === "output" || outputIndexOf(id) !== undefined || routeNodeOf(id)?.output !== undefined;

/** A wire's key, as a tap names it. */
export const wireKey = (edge: FlowEdge): string => `${edge.from}>${edge.to}`;

/**
 * How many messages crossed a wire: what the step it enters received (PL-67), the read for a
 * wire out of a source into the lane, the mapped records for a wire into an output. A trace
 * without `stages` (an older harness) gives the read and the mapped count only.
 */
export function wireCount(trace: Trace, nodes: FlowNode[], edge: FlowEdge): number | undefined {
  if (isOutput(edge.to)) return trace.mapping.length;
  const lane = nodes.filter((node) => isLane(node.id));
  const at = lane.findIndex((node) => node.id === edge.to);
  if (at === 0) return trace.input.events;
  return trace.stages?.find((stage) => stage.step === at)?.reached;
}

export interface Change {
  path: string;
  kind: "added" | "removed" | "changed";
  before?: unknown;
  after?: unknown;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** What a node did to a message, field by field: added, removed or changed, by path. */
export function diffOf(before: unknown, after: unknown, path = ""): Change[] {
  if (isRecord(before) && isRecord(after)) {
    const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])];
    return keys.flatMap((key) => {
      const at = path ? `${path}.${key}` : key;
      if (!(key in before)) return [{ path: at, kind: "added" as const, after: after[key] }];
      if (!(key in after)) return [{ path: at, kind: "removed" as const, before: before[key] }];
      return diffOf(before[key], after[key], at);
    });
  }
  if (JSON.stringify(before) === JSON.stringify(after)) return [];
  if (before === undefined) return [{ path: path || "(message)", kind: "added", after }];
  if (after === undefined) return [{ path: path || "(message)", kind: "removed", before }];
  return [{ path: path || "(message)", kind: "changed", before, after }];
}

/** The most messages step mode walks through; a longer sample is cut there, and says so. */
export const STEP_LIMIT = 50;

/**
 * A sample split into its messages, each a sample of its own the runner reads the same way: a
 * JSON array's items, a CSV's rows each under the header, a text's non-empty lines. A JSON
 * document that is not an array is one message.
 */
export function messagesOf(text: string, format: SampleFormat): string[] {
  if (format === "json") {
    try {
      const parsed: unknown = JSON.parse(text);
      return Array.isArray(parsed) ? parsed.slice(0, STEP_LIMIT).map((item) => JSON.stringify([item])) : [text];
    } catch {
      return [text];
    }
  }
  const lines = text.split(/\r?\n/).filter((line) => line.trim() !== "");
  if (format === "csv") {
    const [header, ...rows] = lines;
    return header === undefined ? [] : rows.slice(0, STEP_LIMIT).map((row) => `${header}\n${row}`);
  }
  return lines.slice(0, STEP_LIMIT);
}

/**
 * The pipeline cut to one node, for a rerun on the input a person pinned: that step alone, or the
 * compute step alone, between the same source and outputs. A source or an output has nothing of
 * its own to rerun.
 */
export function onlyStep(form: PipelineForm, id: FlowNodeId): PipelineForm | undefined {
  if (id === "compute") return form.compute ? { ...form, processors: [] } : undefined;
  const entry = form.processors?.[stepIndexOf(id) ?? -1];
  if (!entry) return undefined;
  const rest: PipelineForm = { ...form, processors: [{ step: entry.step }] };
  delete rest.compute;
  return rest;
}
