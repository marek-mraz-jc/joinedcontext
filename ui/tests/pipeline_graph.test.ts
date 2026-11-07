/**
 * The graph edits of the pipeline studio (PL-56, PL-68, T-3221): a wire only sets the order, a
 * wire the runner cannot run is refused with its reason, and spec -> graph -> spec loses nothing
 * for every stage kind.
 */
import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { toEnvelope, toForm } from "../src/pages/pipelines/PipelineEditor";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { toFlow } from "../src/pages/pipelines/PipelineFlow";
import { connect, laneOf, missingOf, move } from "../src/pages/pipelines/pipelineGraph";

const PROJECT = "banskabystrica";
const ENDPOINT = "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:ovzdusie-write";

/** A v1alpha2 pipeline with `compute` as its compute step between a log and a mapping. */
function pipeline(compute: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha2",
    kind: "Pipeline",
    metadata: { name: "air", namespace: PROJECT },
    spec: {
      class: "auto",
      sources: [
        { dataSourceRef: { kind: "DataSource", name: "eea-pm10" } },
        { endpointRef: { kind: "Endpoint", name: "ovzdusie-read" }, query: { type: "AirQualityObserved" } },
      ],
      steps: [
        { processor: { log: { message: "in" } } },
        compute,
        { processor: { mapping: "root = this" } },
      ],
      outputs: [
        { targetEndpoint: ENDPOINT, mode: "upsert" },
        { targetEndpoint: `${ENDPOINT}-2`, mode: "upsert" },
      ],
    },
  };
}

const COMPUTE_KINDS: Record<string, Record<string, unknown>> = {
  bloblang: { kind: "bloblang", bloblang: "root = this" },
  mapping: { kind: "mapping", mappingRef: { kind: "Mapping", name: "air-to-aqo" } },
  wasm: { kind: "wasm", module: "oci://registry/air:1", function: "map" },
  container: { kind: "container", module: "oci://registry/air-job:1" },
};

const spec = (form: PipelineForm, base: Manifest) => toEnvelope(PROJECT, form, base).spec;

describe("spec -> graph -> spec (PL-56)", () => {
  it.each(Object.entries(COMPUTE_KINDS))("keeps a pipeline with a %s step byte for byte", (_, compute) => {
    const manifest = pipeline(compute);
    const form = toForm(manifest);
    expect(toFlow(form).nodes.map((node) => node.id)).toEqual([
      "source",
      "source-0",
      "step-0",
      "compute",
      "step-1",
      "output",
      "output-0",
    ]);
    expect(spec(form, manifest)).toEqual(manifest.spec);
  });

  it("keeps the spec when a wire moves a step and a second wire moves it back", () => {
    const manifest = pipeline(COMPUTE_KINDS.bloblang);
    const form = toForm(manifest);
    const there = connect(form, "step-1", "step-0");
    if (!("form" in there)) throw new Error("the wire was not taken");
    expect(laneOf(there.form)).toEqual(["compute", "step-0", "step-1"]);
    expect((spec(there.form, manifest) as { steps: unknown[] }).steps).toEqual([
      COMPUTE_KINDS.bloblang,
      { processor: { mapping: "root = this" } },
      { processor: { log: { message: "in" } } },
    ]);
    const back = connect(there.form, "source", there.id);
    if (!("form" in back)) throw new Error("the wire back was not taken");
    expect(spec(back.form, manifest)).toEqual(manifest.spec);
  });
});

describe("a wire only sets the order (PL-56)", () => {
  const form = toForm(pipeline(COMPUTE_KINDS.bloblang));

  it("moves the step it ends at behind the node it starts from, and names where it went", () => {
    const moved = connect(form, "step-0", "step-1");
    expect(moved).toEqual({ form: expect.anything(), id: "step-1" });
    if ("form" in moved) expect(laneOf(moved.form)).toEqual(["step-0", "step-1", "compute"]);
  });

  it("puts a step a source wires into at the head of the lane", () => {
    const moved = connect(form, "source-0", "compute");
    if (!("form" in moved)) throw new Error("the wire was not taken");
    expect(laneOf(moved.form)).toEqual(["compute", "step-0", "step-1"]);
    expect(moved.id).toBe("compute");
  });

  it("changes nothing for a wire the lane already has", () => {
    expect(connect(form, "step-0", "compute")).toEqual({ unchanged: true });
    expect(connect(form, "source", "step-0")).toEqual({ unchanged: true });
    expect(connect(form, "step-1", "output-0")).toEqual({ unchanged: true });
  });

  it("refuses what the runner cannot run, with the reason", () => {
    expect(connect(form, "step-0", "source")).toEqual({ refused: "intoSource" });
    expect(connect(form, "output", "step-0")).toEqual({ refused: "outOfOutput" });
    expect(connect(form, "compute", "compute")).toEqual({ refused: "self" });
    expect(connect(form, "step-0", "output")).toEqual({ refused: "middleIntoOutput" });
  });

  it("lets a source wire straight into an output only while the lane is empty", () => {
    const bare: PipelineForm = { class: "auto", source: { dataSourceRef: "eea-pm10" }, targetEndpoint: ENDPOINT };
    expect(connect(bare, "source", "output")).toEqual({ unchanged: true });
    expect(connect(form, "source", "output")).toEqual({ refused: "middleIntoOutput" });
  });
});

describe("the list view's moves (PL-68)", () => {
  const form = toForm(pipeline(COMPUTE_KINDS.bloblang));

  it("moves a node one place and nothing past either end", () => {
    const later = move(form, "step-0", 1);
    expect(later && laneOf(later.form)).toEqual(["compute", "step-0", "step-1"]);
    expect(later?.id).toBe("step-0");
    expect(move(form, "step-0", -1)).toBeUndefined();
    expect(move(form, "step-1", 1)).toBeUndefined();
    expect(move(form, "source", 1)).toBeUndefined();
    expect(move(undefined, "step-0", 1)).toBeUndefined();
  });
});

describe("a node's state before a test (PL-68)", () => {
  it("names what a node still lacks, and nothing for a node that has it", () => {
    const empty: PipelineForm = {
      class: "auto",
      moreSources: [{}],
      moreOutputs: [{ mode: "upsert" }],
      compute: { kind: "bloblang", bloblang: "  " },
      processors: [{ step: { processor: { mapping: "" } } }, { step: { processor: { log: {} } } }],
    };
    expect(missingOf(empty, "source")).toBe("source");
    expect(missingOf(empty, "source-0")).toBe("source");
    expect(missingOf(empty, "output")).toBe("output");
    expect(missingOf(empty, "output-0")).toBe("output");
    expect(missingOf(empty, "compute")).toBe("mapping");
    expect(missingOf(empty, "step-0")).toBe("mapping");
    expect(missingOf(empty, "step-1")).toBeUndefined();
    expect(missingOf({ class: "auto", compute: { kind: "mapping" } }, "compute")).toBe("mappingRef");
    expect(missingOf({ class: "auto", compute: { kind: "wasm" } }, "compute")).toBe("module");

    const full = toForm(pipeline(COMPUTE_KINDS.bloblang));
    for (const id of toFlow(full).nodes.map((node) => node.id)) expect(missingOf(full, id)).toBeUndefined();
  });
});
