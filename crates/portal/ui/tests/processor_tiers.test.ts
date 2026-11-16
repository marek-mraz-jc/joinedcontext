/** T-7062 (G-23): every processor the runner builds sits in exactly one palette tier. */
import { describe, expect, it } from "vitest";
import tiers from "../src/schemas/processor-tiers.json";
import { RUNNER_PROCESSORS } from "../src/schemas/runner";
import { PROCESSOR_TIERS } from "../src/pages/pipelines/PipelineFlow";

describe("the processor tiers", () => {
  it("place each runner processor in one tier and name none the runner lacks", () => {
    const listed = [...tiers.forms, ...tiers.advanced, ...tiers.bloblang];
    expect(new Set(listed).size).toBe(listed.length);
    expect([...RUNNER_PROCESSORS].filter((name) => !listed.includes(name))).toEqual([]);
    expect(listed.filter((name) => !RUNNER_PROCESSORS.has(name))).toEqual([]);
  });

  it("offer the forms tier first, then advanced, then Bloblang", () => {
    expect(PROCESSOR_TIERS.map(({ tier }) => tier)).toEqual(["forms", "advanced", "bloblang"]);
    expect(PROCESSOR_TIERS[0].processors.map(({ name }) => name)).toContain("dedupe");
    expect(PROCESSOR_TIERS[2].processors.map(({ name }) => name)).toContain("mapping");
  });
});
