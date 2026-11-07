/**
 * Formula fields (DM-80, T-3133): the `equals_expression` subset parsed, checked against the class,
 * compiled to Bloblang from its tokens alone, and the pipeline and Policies it runs by. The
 * evaluator's results are the ones the compiled Bloblang gave in Bento (bento 1.21.1, recorded in
 * T-3133) on the same rows.
 */
import { describe, expect, it } from "vitest";
import {
  checkFormulas,
  evaluate,
  formulaManifests,
  formulaMapping,
  parseFormula,
  pipelineName,
  rangeOf,
  refsOf,
  toBloblang,
} from "../src/components/entities/formula";
import type { ClassSlot } from "../src/components/entities/formula";

const SLOTS: ClassSlot[] = [
  { name: "available", kind: "Property" },
  { name: "capacity", kind: "Property" },
  { name: "label", kind: "Property" },
  { name: "location", kind: "GeoProperty" },
  { name: "share", kind: "Property", equals_expression: "round({available} / {capacity} * 100)" },
  { name: "full", kind: "Property", equals_expression: "{share} >= 90 and not ({capacity} == 0)" },
  { name: "title", kind: "Property", equals_expression: "{label} + ' (' + 'ok' + ')'" },
  { name: "spare", kind: "Property", equals_expression: "max({capacity} - {available}, 0) % 7 - -1" },
];

describe("parsing", () => {
  it("reads precedence, functions and references", () => {
    const formula = parseFormula("1 + {a} * 2 >= {b} or not {c}");
    expect(formula).toEqual({
      kind: "binary",
      op: "or",
      left: {
        kind: "binary",
        op: ">=",
        left: { kind: "binary", op: "+", left: { kind: "number", value: 1 }, right: { kind: "binary", op: "*", left: { kind: "ref", name: "a" }, right: { kind: "number", value: 2 } } },
        right: { kind: "ref", name: "b" },
      },
      right: { kind: "unary", op: "not", arg: { kind: "ref", name: "c" } },
    });
    expect(refsOf(parseFormula("min({a}, {b}) + abs({a})"))).toEqual(["a", "b"]);
  });

  it("refuses anything outside the subset, saying where", () => {
    for (const [text, said] of [
      ["", "empty"],
      ["{a} +", "ends too early"],
      ["{a} {b}", "something follows"],
      ['env("HOME")', "not a function of a formula"],
      ["this.capacity", "`.` is not part of a formula"],
      ["{a} ; root = 1", "not part of a formula"],
      ["{a.b}", "slot name in braces"],
      ["'open", "not closed"],
      ["min({a})", "min takes 2 values"],
      ["10000000000000000", "too large"],
      ["x".repeat(1001), "at most 1000"],
    ] as const) {
      expect(() => parseFormula(text), text).toThrow(said);
    }
  });

  it("says what range a formula's value is", () => {
    expect(rangeOf(parseFormula("{a} / 2"))).toBe("float");
    expect(rangeOf(parseFormula("{a} > 2"))).toBe("boolean");
    expect(rangeOf(parseFormula("{a} + ' kWh'"))).toBe("string");
  });
});

describe("checking a class's formulas", () => {
  it("orders each formula after those it reads", () => {
    const { order, problems } = checkFormulas(SLOTS);
    expect(problems).toEqual({});
    const names = order.map((entry) => entry.name);
    expect(names.indexOf("share")).toBeLessThan(names.indexOf("full"));
    expect(names).toHaveLength(4);
  });

  it("refuses a formula slot whose name is not a plain name, so nothing else reaches the mapping", () => {
    // F-UI1-022: the slot name is spliced into `let v_{slot}` and `root.{slot}`.
    const evil = "a } root = deleted() {";
    const checked = checkFormulas([
      { name: "capacity", kind: "Property" },
      { name: evil, kind: "Property", equals_expression: "{capacity} * 2" },
    ]);
    expect(checked.problems[evil]).toBe("a formula slot's name is letters, digits and _, such as free_places");
    expect(checked.order).toEqual([]);
    expect(formulaMapping(checked, "Parking")).not.toContain("deleted() {");
  });

  it("refuses a slot outside the class, a geometry, and a cycle naming every slot on it", () => {
    const { problems, order } = checkFormulas([
      { name: "a", kind: "Property", equals_expression: "{b} + 1" },
      { name: "b", kind: "Property", equals_expression: "{c} * 2" },
      { name: "c", kind: "Property", equals_expression: "{a} - 1" },
      { name: "d", kind: "Property", equals_expression: "{nowhere}" },
      { name: "e", kind: "Property", equals_expression: "{location}" },
      { name: "location", kind: "GeoProperty" },
      { name: "f", kind: "Property", equals_expression: "{d} + 1" },
    ]);
    expect(problems.a).toBe("a cycle: a → b → c → a");
    expect(problems.b).toBe("a cycle: a → b → c → a");
    expect(problems.c).toBe("a cycle: a → b → c → a");
    expect(problems.d).toBe("{nowhere} is not a slot of this class");
    expect(problems.e).toBe("{location} is a GeoProperty, which a formula cannot compute with");
    expect(problems.f).toBe("it reads a formula that cannot be computed");
    expect(order).toEqual([]);
  });
});

describe("compiling", () => {
  it("builds Bloblang from the tokens alone", () => {
    expect(toBloblang(parseFormula("min({a}, 2) + abs(-{b}) / round({c})"))).toBe(
      "((if $v_a < 2 { $v_a } else { 2 }) + (((0 - $v_b)).abs() / ($v_c).round()))",
    );
    expect(toBloblang(parseFormula("{a} and not {b} or {c} != 'x'"))).toBe('(($v_a && !($v_b)) || ($v_c != "x"))');
    // A quote inside text is escaped as JSON, so text can never close the literal.
    expect(toBloblang(parseFormula(`'say "hi"'`))).toBe('"say \\"hi\\""');
  });

  it("writes `$` so the runner fills no variable of its environment into a text (T-6230)", () => {
    // `${JC_CLIENT_SECRET}` passes the server's check as the project's own credential, and Bento
    // fills it in the stream text before Bloblang reads it (bento 1.2.0, evidence/T-6230).
    const slots: ClassSlot[] = [
      { name: "label", kind: "Property" },
      { name: "leak", kind: "Property", equals_expression: "{label} + '${JC_CLIENT_SECRET}' + \"${!hostname()}\"" },
    ];
    const mapping = formulaMapping(checkFormulas(slots), "Station");
    expect(mapping).not.toContain("${");
    expect(mapping).toContain('"\\u0024{JC_CLIENT_SECRET}"');
    expect(evaluate(parseFormula("'${JC_CLIENT_SECRET}'"), {})).toBe("${JC_CLIENT_SECRET}");
  });

  it("maps each entity of the type into its formula attributes with computedBy", () => {
    const mapping = formulaMapping(checkFormulas(SLOTS), "Station");
    expect(mapping).toContain('let pipeline = "urn:ngsi-ld:Pipeline:%v:%v:formulas-station".format(env("JC_ORG_DOMAIN"), env("JC_SPACE"))');
    expect(mapping).toContain("let v_available = this.available.value.catch(null)");
    expect(mapping).toContain("let v_share = if [$v_available, $v_capacity].contains(null) { null } else {");
    expect(mapping.indexOf("let v_share")).toBeLessThan(mapping.indexOf("let v_full"));
    expect(mapping).toContain('root.share = if $v_share == null { deleted() } else { { "type": "Property", "value": $v_share, "computedBy": { "type": "Property", "value": $pipeline } } }');
    expect(mapping).toContain('root = this.or([]).filter(e -> e.type == "Station")');
    // The only environment it reads is the runner's own domain and space, for computedBy.
    expect(mapping.match(/env\(/g)).toHaveLength(2);
  });

  it("computes on a row what the compiled Bloblang computed in Bento", () => {
    const formulas = Object.fromEntries(checkFormulas(SLOTS).order.map((entry) => [entry.name, entry.formula]));
    const row = (values: Record<string, unknown>) => {
      const out: Record<string, unknown> = { ...values };
      for (const name of ["share", "full", "title", "spare"]) out[name] = evaluate(formulas[name], out);
      return out;
    };
    expect(row({ available: 9, capacity: 10, label: "Kamppi" })).toMatchObject({ share: 90, full: true, spare: 2, title: "Kamppi (ok)" });
    expect(row({ available: 3, capacity: 0 })).toMatchObject({ share: null, full: null, spare: 1, title: null });
    expect(row({ label: "Kallio" })).toMatchObject({ share: null, title: "Kallio (ok)" });
    expect(row({ available: "many", capacity: 4 })).toMatchObject({ share: null, spare: null });
    expect(evaluate(parseFormula("round({a})"), { a: -2.5 })).toBe(-3);
  });
});

describe("the manifests the formulas run by", () => {
  it("are a scheduled pipeline through the space's Endpoint and two Policies of exactly what it reads and writes", () => {
    const [read, write, pipeline] = formulaManifests(
      { project: "helsinki", space: "bikes", type: "Station", orgDomain: "hel.fi", endpoint: "bikes-all" },
      checkFormulas(SLOTS),
    );
    expect(pipelineName("Station")).toBe("formulas-station");
    // Every name, the Policies' suffixed ones too, stays a DNS-1123 label of at most 63 (T-6230).
    const long = formulaManifests(
      { project: "helsinki", space: "bikes", type: "A".repeat(60), orgDomain: "hel.fi", endpoint: "bikes-all" },
      checkFormulas(SLOTS),
    );
    for (const manifest of long) expect(manifest.metadata.name.length).toBeLessThanOrEqual(63);
    expect(pipeline).toMatchObject({
      kind: "Pipeline",
      metadata: { name: "formulas-station", namespace: "helsinki" },
      spec: {
        class: "scheduled",
        source: { endpointRef: { kind: "Endpoint", name: "bikes-all" }, query: { type: "Station" } },
        targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-all",
        output: { type: "Station", mode: "update-attrs" },
      },
    });
    expect(read).toMatchObject({
      kind: "Policy",
      metadata: { name: "formulas-station-read" },
      spec: { assignee: { kind: "serviceAccount", id: "pipelines" }, operations: ["queryEntity"] },
    });
    expect(write).toMatchObject({ spec: { operations: ["upsertBatch"], information: [{ entities: [{ type: "Station" }] }] } });
    // Written: the formula slots and nothing else; read: what they compute from, and themselves.
    const names = (manifest: typeof write) => (manifest.spec as { information: { propertyNames: string[] }[] }).information[0].propertyNames;
    expect(names(write)).toEqual(["share", "full", "title", "spare"]);
    expect(names(read)).toEqual(["available", "capacity", "share", "label", "full", "title", "spare"]);
  });
});
