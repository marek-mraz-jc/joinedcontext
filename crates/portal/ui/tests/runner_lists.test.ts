import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import bentoInputs from "../src/schemas/bento-inputs.json";
import ekibenInputs from "../src/schemas/ekiben-inputs.json";
import { runnerCatalog } from "../src/pages/datasources/RunnerInputForm";
import type { BentoCatalog } from "../src/pages/datasources/RunnerInputForm";
import type { CatalogInput } from "../src/schemas/kinds";
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

  // T-7092: ekiben's own inputs (ftp, imap) are in the picker, and their credential fields are
  // the ones the server holds to a secret reference (jc-core EKIBEN_ONLY_INPUTS).
  it("offer ekiben's own inputs with the secret fields the server checks", () => {
    const catalog = runnerCatalog(bentoInputs as BentoCatalog, ekibenInputs as { inputs: CatalogInput[] });
    const names = catalog.inputs.map(({ name }) => name);
    expect(names).toEqual(expect.arrayContaining(["ftp", "imap", "http_client"]));
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b)));
    const table = /const EKIBEN_ONLY_INPUTS[^=]*=\s*&\[([\s\S]*?)\];/.exec(readFileSync(join(KINDS, "data_source.rs"), "utf8"));
    const server = Object.fromEntries(
      [...(table?.[1] ?? "").matchAll(/\("([a-z0-9_]+)", &\[([^\]]*)\]/g)].map(([, name, paths]) => [name, [...paths.matchAll(/"([^"]+)"/g)].map(([, p]) => p)]),
    );
    const form = Object.fromEntries(
      (ekibenInputs.inputs as CatalogInput[]).map((input) => [input.name, input.fields.filter((f) => f.secret).map((f) => f.path)]),
    );
    expect(form).toEqual(server);
    for (const name of Object.keys(form)) expect(RUNNER_INPUTS.has(name), name).toBe(true);
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
