/**
 * One plain-language model editor (T-4829): IRIs minted inside the organisation's prefix, the
 * KPI diagram without an `Any` box or a URN key on a value object, and fields in plain words.
 */
import { describe, expect, it } from "vitest";
import { ANY_VALUE, graphData, mintIri, parseModel, valueClasses } from "../src/pages/models/linkml";
import { applyOperations } from "../src/pages/models/operations";
import { fieldWord } from "../src/components/entities/fieldTypes";
import { neighbourhood } from "../src/pages/models/diagramReading";

const ORG = `id: https://hel.fi/models/kpi
name: kpi
prefixes:
  hel: https://hel.fi/terms/
  linkml: https://w3id.org/linkml/
  sdm: https://smartdatamodels.org/
default_prefix: hel
default_range: string
classes:
  Building:
    class_uri: hel:Building
    slots: [name]
slots:
  name:
    slot_uri: hel:name
`;

/** The shape of the Helsinki KPI seed: `Any` and an inline period, as B05 drew them wrong. */
const KPI = `id: https://hel.fi/models/kpi
name: kpi
prefixes:
  jc: https://hel.fi/terms/
  linkml: https://w3id.org/linkml/
default_prefix: jc
default_range: string
slots:
  name: { slot_uri: jc:name }
  currentValue: { slot_uri: jc:currentValue, range: Any, inlined: true }
  calculationPeriod: { slot_uri: jc:calculationPeriod, range: CalculationPeriod, inlined: true }
  startsAt: { slot_uri: jc:startsAt, range: datetime }
  endsAt: { slot_uri: jc:endsAt, range: datetime }
  district: { slot_uri: jc:district, range: District }
classes:
  KeyPerformanceIndicator:
    class_uri: jc:KeyPerformanceIndicator
    slots: [name, currentValue, calculationPeriod, district]
  District:
    class_uri: jc:District
    slots: [name]
  CalculationPeriod:
    class_uri: jc:CalculationPeriod
    slots: [startsAt, endsAt]
  Any:
    class_uri: linkml:Any
`;

describe("IRIs a steward never types", () => {
  it("mints a new type and a new field under the organisation's default prefix", () => {
    const added = applyOperations(ORG, [
      { op: "addClass", name: "School" },
      { op: "addSlot", name: "pupils", class: "School", range: "integer" },
    ]);
    expect(added.refused).toEqual([]);
    const model = parseModel(added.source);
    expect(model.classes.find((one) => one.name === "School")?.class_uri).toBe("hel:School");
    expect(model.slots.find((one) => one.name === "pupils")?.slot_uri).toBe("hel:pupils");
  });

  it("keeps an IRI the caller gives", () => {
    const added = applyOperations(ORG, [{ op: "addClass", name: "School", class_uri: "hel:Koulu" }]);
    expect(parseModel(added.source).classes.find((one) => one.name === "School")?.class_uri).toBe("hel:Koulu");
  });

  it("never mints outside the organisation: a reserved or undeclared default prefix mints nothing", () => {
    const reserved = parseModel(ORG.replace("default_prefix: hel", "default_prefix: sdm"));
    expect(mintIri(reserved, "School")).toBeUndefined();
    const undeclared = parseModel(ORG.replace("default_prefix: hel", "default_prefix: elsewhere"));
    expect(mintIri(undeclared, "School")).toBeUndefined();
    const none = parseModel(ORG.replace("default_prefix: hel\n", ""));
    expect(mintIri(none, "School")).toBeUndefined();
    // A name that is no identifier cannot smuggle a path or another namespace into the IRI.
    expect(mintIri(parseModel(ORG), "x/../y")).toBeUndefined();
    expect(mintIri(parseModel(ORG), "https://evil.example/x")).toBeUndefined();
  });

  it("a class added without a default prefix still says it needs an IRI", () => {
    const added = applyOperations(ORG.replace("default_prefix: hel\n", ""), [{ op: "addClass", name: "School" }]);
    expect(parseModel(added.source).classes.find((one) => one.name === "School")?.class_uri).toBeUndefined();
  });
});

describe("the KPI diagram", () => {
  const model = parseModel(KPI);
  const { nodes, edges } = graphData(model);

  it("draws no box for Any and none for a value object", () => {
    expect(nodes.map((node) => node.name).sort()).toEqual(["District", "KeyPerformanceIndicator"]);
    expect(valueClasses(model)).toEqual(new Set(["CalculationPeriod"]));
  });

  it("draws a line only where the range is a class an entity points at", () => {
    expect(edges.map((edge) => `${edge.from}->${edge.to}`)).toEqual(["KeyPerformanceIndicator->District"]);
    expect(neighbourhood(nodes.map((node) => node.name), edges, "KeyPerformanceIndicator", 1)).toEqual(
      new Set(["KeyPerformanceIndicator", "District"]),
    );
  });

  it("shows Any as any value and the period's fields inside the KPI box, no FK and no URN", () => {
    const rows = nodes.find((node) => node.name === "KeyPerformanceIndicator")?.rows ?? [];
    const current = rows.find((row) => row.name === "currentValue");
    expect(current).toMatchObject({ type: ANY_VALUE });
    expect(current?.key).toBeUndefined();
    const period = rows.find((row) => row.name === "calculationPeriod");
    expect(period).toMatchObject({ type: "CalculationPeriod", inline: ["startsAt", "endsAt"] });
    expect(period?.key).toBeUndefined();
    expect(rows.find((row) => row.name === "district")?.key).toBe("fk");
    expect(nodes.flatMap((node) => node.rows ?? []).filter((row) => row.type === "URN")).toHaveLength(2);
  });

  it("a class both inlined and referenced stays a box with its key", () => {
    const shared = parseModel(KPI.replace("district: { slot_uri: jc:district, range: District }", "district: { slot_uri: jc:district, range: CalculationPeriod }"));
    expect(valueClasses(shared).has("CalculationPeriod")).toBe(false);
    expect(graphData(shared).nodes.some((node) => node.name === "CalculationPeriod")).toBe(true);
  });
});

describe("fields in plain words", () => {
  const model = parseModel(KPI);
  const slot = (name: string) => {
    const found = model.slots.find((one) => one.name === name);
    if (!found) throw new Error(name);
    return found;
  };
  it("names each field as the Add field dialog does", () => {
    expect(fieldWord(slot("name"), model)).toEqual({ type: "text" });
    expect(fieldWord(slot("startsAt"), model)).toEqual({ type: "datetime" });
    expect(fieldWord(slot("currentValue"), model)).toEqual({ type: "any" });
    expect(fieldWord(slot("calculationPeriod"), model)).toEqual({ type: "object", target: "CalculationPeriod", fields: ["startsAt", "endsAt"] });
    expect(fieldWord(slot("district"), model)).toEqual({ type: "relationship", target: "District" });
  });
});
