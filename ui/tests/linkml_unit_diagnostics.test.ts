/**
 * What the editor says about a slot's unit (DM-06, DM-59, T-2809): an unknown code, a unit on
 * something that is not a number, a measured quantity without a unit, and one IRI in two units.
 */
import { describe, expect, it } from "vitest";
import { diagnose, unitCode } from "../src/pages/models/linkml";

function model(slots: string): string {
  return `id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots: [${[...slots.matchAll(/^ {2}(\w+):$/gm)].map((m) => m[1]).join(", ")}]
slots:
${slots}`;
}

function unitMessages(source: string) {
  return diagnose(source)
    .filter((d) => /unit/.test(d.message))
    .map((d) => ({ severity: d.severity, path: d.path, message: d.message }));
}

describe("unit diagnostics", () => {
  it("says nothing about a number in a known unit", () => {
    const source = model(`  pm10:
    range: float
    slot_uri: bb:pm10
    unit:
      ucum_code: ug.m-3
      exact_mappings: ["ucefact:GQ", "qudt-unit:MicroGM-PER-M3"]
`);
    expect(unitMessages(source)).toEqual([]);
  });

  it("refuses a code that is not in Recommendation 20", () => {
    const source = model(`  pm10:
    range: float
    slot_uri: bb:pm10
    unit:
      exact_mappings: ["ucefact:UGM3"]
`);
    expect(unitMessages(source)).toEqual([
      {
        severity: "error",
        path: "slots.pm10",
        message: "slot 'pm10' declares unit 'UGM3', which is not a UN/CEFACT Recommendation 20 code; pick one from the unit list",
      },
    ]);
  });

  it("refuses a unit on a slot that is not a number", () => {
    const source = model(`  label:
    range: string
    slot_uri: bb:label
    unit:
      exact_mappings: ["ucefact:CEL"]
`);
    expect(unitMessages(source)).toEqual([
      {
        severity: "error",
        path: "slots.label",
        message:
          "slot 'label' declares a unit but its range is 'string'; a unit belongs on a number (integer, float, double or decimal)",
      },
    ]);
  });

  it("warns about a number that reads as a measured quantity and has no unit, and about nothing else", () => {
    const source = model(`  temperature:
    range: float
    slot_uri: bb:temperature
  stationCount:
    range: integer
    slot_uri: bb:stationCount
  speedLimitLabel:
    range: string
    slot_uri: bb:speedLimitLabel
`);
    expect(unitMessages(source)).toEqual([
      {
        severity: "warning",
        path: "slots.temperature",
        message:
          "slot 'temperature' is a number that reads as a measured quantity but declares no unit; pick one, or every reader guesses",
      },
    ]);
  });

  it("refuses two slots bound to one IRI in different units, and lets the same unit pass", () => {
    const slot = (name: string, code: string) => `  ${name}:
    range: float
    slot_uri: bb:temperature
    unit:
      exact_mappings: ["ucefact:${code}"]
`;
    expect(unitMessages(model(slot("tempC", "CEL") + slot("tempF", "FAH")))).toEqual([
      {
        severity: "error",
        path: "slots.tempF",
        message:
          "slots 'tempC' and 'tempF' are both bound to 'bb:temperature' but measure in CEL and FAH; one IRI takes one unit",
      },
    ]);
    expect(unitMessages(model(slot("tempA", "CEL") + slot("tempB", "CEL")))).toEqual([]);
  });

  it("reads the CEFACT code from its own mapping wherever it stands, and the older unece: spelling", () => {
    expect(unitCode({ exact_mappings: ["qudt-unit:DEG_C", "ucefact:CEL"] })).toBe("CEL");
    expect(unitCode({ exact_mappings: ["unece:CEL"] })).toBe("CEL");
    expect(unitCode({ exact_mappings: ["qudt-unit:DEG_C"] })).toBeUndefined();
    expect(unitCode(undefined)).toBeUndefined();
  });
});
