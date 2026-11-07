/**
 * T-3222: what crossed each wire, what a node changed, a sample cut into its messages for step
 * mode, and the pipeline cut to one step for a rerun on a pinned input (PL-43, PL-67).
 */
import { describe, expect, it } from "vitest";
import type { Trace } from "../src/api/pipelineTest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { toFlow } from "../src/pages/pipelines/PipelineFlow";
import { STEP_LIMIT, diffOf, messagesOf, onlyStep, wireCount, wireKey } from "../src/pages/pipelines/pipelineDebug";

const FORM: PipelineForm = {
  class: "auto",
  source: { dataSourceRef: "feed" },
  processors: [{ step: { processor: { log: {} } } }, { step: { processor: { jq: { query: "." } } }, after: true }],
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:ep",
};

const TRACE: Trace = {
  input: { events: 5, bytes: 100 },
  mapping: [{}, {}, {}],
  validation: [],
  errors: [],
  stages: [
    { step: 0, reached: 5 },
    { step: 1, reached: 4 },
    { step: 2, reached: 3 },
  ],
};

describe("counts on the wires", () => {
  const { nodes, edges } = toFlow(FORM);

  it("counts what the step a wire enters received, the read first and the mapped last", () => {
    expect(edges.map((edge) => [wireKey(edge), wireCount(TRACE, nodes, edge)])).toEqual([
      ["source>step-0", 5],
      ["step-0>compute", 4],
      ["compute>step-1", 3],
      ["step-1>output", 3],
    ]);
  });

  it("says nothing for a middle wire when the harness gave no stages", () => {
    const old = { ...TRACE, stages: undefined };
    expect(wireCount(old, nodes, edges[1])).toBeUndefined();
    expect(wireCount(old, nodes, edges[0])).toBe(5);
  });
});

describe("what a node changed", () => {
  it("names each field added, removed or changed, by path", () => {
    expect(diffOf({ id: "1", pm: { ten: 4 }, gone: true }, { id: "1", pm: { ten: 5 }, type: "A" })).toEqual([
      { path: "pm.ten", kind: "changed", before: 4, after: 5 },
      { path: "gone", kind: "removed", before: true },
      { path: "type", kind: "added", after: "A" },
    ]);
    expect(diffOf({ a: [1] }, { a: [1] })).toEqual([]);
    expect(diffOf("x", "y")).toEqual([{ path: "(message)", kind: "changed", before: "x", after: "y" }]);
    expect(diffOf(undefined, { a: 1 })).toEqual([{ path: "(message)", kind: "added", after: { a: 1 } }]);
  });
});

describe("one message at a time", () => {
  it("cuts JSON, CSV and text into messages the runner reads the same way", () => {
    expect(messagesOf('[{"a":1},{"a":2}]', "json")).toEqual(['[{"a":1}]', '[{"a":2}]']);
    expect(messagesOf('{"a":1}', "json")).toEqual(['{"a":1}']);
    expect(messagesOf("not json", "json")).toEqual(["not json"]);
    expect(messagesOf("id,pm\n1,4\r\n\n2,5\n", "csv")).toEqual(["id,pm\n1,4", "id,pm\n2,5"]);
    expect(messagesOf("", "csv")).toEqual([]);
    expect(messagesOf("one\n\ntwo", "text")).toEqual(["one", "two"]);
  });

  it("stops at the step limit", () => {
    const many = JSON.stringify(Array.from({ length: STEP_LIMIT + 10 }, (_, at) => ({ at })));
    expect(messagesOf(many, "json")).toHaveLength(STEP_LIMIT);
  });
});

describe("one step rerun on a pinned input", () => {
  it("keeps only that step between the same source and output", () => {
    expect(onlyStep(FORM, "step-1")).toEqual({
      class: "auto",
      source: FORM.source,
      processors: [{ step: { processor: { jq: { query: "." } } } }],
      targetEndpoint: FORM.targetEndpoint,
    });
    expect(onlyStep(FORM, "compute")).toEqual({ ...FORM, processors: [] });
    expect(onlyStep(FORM, "source")).toBeUndefined();
    expect(onlyStep(FORM, "output")).toBeUndefined();
    expect(onlyStep(FORM, "step-9")).toBeUndefined();
  });
});
