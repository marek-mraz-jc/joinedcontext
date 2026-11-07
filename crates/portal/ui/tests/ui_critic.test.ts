import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  budgetOf,
  critique,
  dedupe,
  judge,
  parseAnswer,
  routeMatches,
  type Ask,
  type Finding,
} from "../e2e/agent/critic";

// T-5982: the UI critic's contract with the model and its bookkeeping. The model itself is judged
// by `node e2e/agent/critic.ts --selftest` over the two committed shots (clean passes, raw JSON high).

const answers = (...texts: string[]): Ask & { calls: number } => {
  const ask: Ask & { calls: number } = Object.assign(
    async () => {
      ask.calls += 1;
      return { answer: texts[Math.min(ask.calls - 1, texts.length - 1)], cost: 0.001 };
    },
    { calls: 0 },
  );
  return ask;
};

const finding = (over: Partial<Finding & { run: string }>): Finding & { run: string } => ({
  run: "r1",
  scenario: "S-62",
  step: 1,
  width: 1440,
  lang: "en",
  shot: "1.png",
  route: "/projects/helsinki/endpoints",
  rule: "raw-json",
  severity: "medium",
  what: "raw JSON in the Area column",
  where: "td.area",
  seen: [],
  ...over,
});

describe("parseAnswer", () => {
  it("reads a fenced answer and trims its fields", () => {
    expect(parseAnswer('```json\n{"findings":[{"rule":"bare-urn","severity":"high","what":" urn shown ","where":"td"}]}\n```')).toEqual([
      { rule: "bare-urn", severity: "high", what: "urn shown", where: "td" },
    ]);
  });

  it("refuses a rule outside the rubric, a severity outside the scale and an empty what", () => {
    expect(() => parseAnswer('{"findings":[{"rule":"vibes","severity":"high","what":"x"}]}')).toThrow(/unknown rule/);
    expect(() => parseAnswer('{"findings":[{"rule":"look","severity":"blocker","what":"x"}]}')).toThrow(/severity/);
    expect(() => parseAnswer('{"findings":[{"rule":"look","severity":"low","what":" "}]}')).toThrow(/no what/);
    expect(() => parseAnswer('{"ok":true}')).toThrow(/no findings array/);
  });
});

describe("judge", () => {
  it("asks once more after an answer off the contract", async () => {
    const ask = answers("not json", '{"findings":[]}');
    expect(await judge(ask, "p", Buffer.from(""))).toEqual({ findings: [], cost: 0.002 });
    expect(ask.calls).toBe(2);
  });

  it("reports the critic's own failure after two bad answers, never an empty pass", async () => {
    const ask = answers("nope");
    const result = await judge(ask, "p", Buffer.from(""));
    expect(ask.calls).toBe(2);
    expect(result.error).toMatch(/off the contract twice/);
  });
});

describe("dedupe", () => {
  it("merges one defect across runs, widths and languages and keeps the worst severity", () => {
    const merged = dedupe(
      [finding({}), finding({ run: "r2", width: 400, lang: "sk", severity: "high", where: " TD.area " }), finding({ rule: "overflow" })],
      [],
    );
    expect(merged).toHaveLength(2);
    expect(merged[0].severity).toBe("high");
    expect(merged[0].seen).toEqual([
      { run: "r1", step: 1, width: 1440, lang: "en" },
      { run: "r2", step: 1, width: 400, lang: "sk" },
    ]);
  });

  it("points at the walker's failing result for the same rule and route", () => {
    const walker = [
      { key: "raw-json /projects/$project/endpoints {\"a\":1}", verdict: "fail", route: "/projects/$project/endpoints" },
      { key: "overflow /projects/$project/endpoints", verdict: "pass", route: "/projects/$project/endpoints" },
    ];
    const [json, overflow] = dedupe([finding({}), finding({ rule: "overflow" })], walker);
    expect(json.walker).toBe(walker[0].key);
    expect(overflow.walker).toBeUndefined();
  });
});

describe("routes and budgets", () => {
  it("matches walker route patterns segment by segment", () => {
    expect(routeMatches("/projects/$project/settings/roles/new", "/projects/helsinki/settings/roles/new")).toBe(true);
    expect(routeMatches("/projects/$project", "/projects/helsinki/endpoints")).toBe(false);
  });

  it("reads the step budget from the scenario's expect line", () => {
    expect(budgetOf({ id: "S-56", expect: "done in <= 6 steps on the phone" })).toBe(6);
    expect(budgetOf({ id: "S-61", expect: "finds own projects" })).toBeNull();
  });
});

describe("critique", () => {
  it("judges every shot of a run and gives the run its verdict", async () => {
    const dir = mkdtempSync(join(tmpdir(), "critic-"));
    writeFileSync(join(dir, "1.png"), "");
    writeFileSync(join(dir, "2.png"), "");
    const steps = [
      { n: 1, url: "https://portal.example/projects/helsinki/endpoints?x=1", shot: "1.png" },
      { n: 2, url: "https://portal.example/projects/helsinki/endpoints", shot: "2.png" },
      { n: 3, action: { done: true } },
    ];
    writeFileSync(join(dir, "run.json"), JSON.stringify({ scenario: "S-56", persona: "citizen", width: 400, lang: "sk", outcome: "done", steps }));
    const ask = answers('{"findings":[{"rule":"overflow","severity":"medium","what":"table cut off","where":"table"}]}');
    const report = await critique([dir], [{ id: "S-56", expect: "done in <= 2 steps" }], [], ask);
    expect(ask.calls).toBe(2);
    expect(report.findings).toHaveLength(1);
    expect(report.findings[0]).toMatchObject({ route: "/projects/helsinki/endpoints", step: 1, width: 400, lang: "sk" });
    expect(report.findings[0].seen).toHaveLength(2);
    expect(report.verdicts[0]).toMatchObject({ finishedAlone: true, steps: 3, budget: 2, withinBudget: false, worst: "medium" });
    expect(report.errors).toEqual([]);
  });
});
