import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import bentoInputs from "../src/schemas/bento-inputs.json";
import { PROCESSOR_GROUPS } from "../src/pages/pipelines/PipelineFlow";
import { REFUSED_INPUTS, REFUSED_PROCESSORS, RUNNER_INPUTS, RUNNER_PROCESSORS, offeredInput } from "../src/schemas/runner";

const KINDS = join(__dirname, "../../../../vendor/joinedcontext-platform/crates/jc-core/src/kinds");

/** The names of a `const <name>: &[&str] = &[...]` list in a jc-core source file. */
function rustList(file: string, name: string): string[] {
  const list = new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(
    readFileSync(join(KINDS, file), "utf8"),
  );
  return [...(list?.[1].matchAll(/"([^"]+)"/g) ?? [])].map((match) => match[1]);
}

describe("runner lists (T-6825)", () => {
  it("are the lists the server checks on save", () => {
    expect([...RUNNER_INPUTS]).toEqual(rustList("ekiben_components.rs", "INPUTS"));
    expect([...RUNNER_PROCESSORS]).toEqual(rustList("ekiben_components.rs", "PROCESSORS"));
    expect([...REFUSED_INPUTS]).toEqual(rustList("data_source.rs", "REFUSED_INPUTS"));
    expect([...REFUSED_PROCESSORS]).toEqual(rustList("pipeline.rs", "REFUSED_PROCESSORS"));
  });

  it("offer only the inputs the runner builds and the platform accepts", () => {
    const offered = bentoInputs.inputs.map(({ name }) => name).filter(offeredInput);
    expect(offered).toContain("http_client");
    expect(offered).toContain("nats");
    expect(offered).toContain("sql_select");
    // T-7090: the runner builds amqp_0_9 and sftp; T-7093: kafka_franz (Bento's older kafka input is not built).
    expect(offered).toContain("amqp_0_9");
    expect(offered).toContain("sftp");
    expect(offered).toContain("kafka_franz");
    for (const name of ["kafka", "mongodb", "stdin", "http_server"]) {
      expect(offered).not.toContain(name);
    }
  });

  it("offer only the processors the runner builds and the platform accepts", () => {
    const offered = PROCESSOR_GROUPS.flatMap(({ processors }) => processors.map(({ name }) => name));
    expect(offered).toContain("mapping");
    expect(offered).toContain("jq");
    expect(offered).toContain("sql_select");
    expect(offered.every((name) => RUNNER_PROCESSORS.has(name) && !REFUSED_PROCESSORS.has(name))).toBe(true);
    // javascript is built by the runner (T-7061) and still refused as a step.
    expect(RUNNER_PROCESSORS.has("javascript")).toBe(true);
    for (const name of ["couchbase", "javascript", "python"]) {
      expect(offered).not.toContain(name);
    }
  });
});
