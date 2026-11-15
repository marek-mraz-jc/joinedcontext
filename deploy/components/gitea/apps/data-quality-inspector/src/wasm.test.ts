import { describe, expect, it } from "vitest";
import wasm from "../wasm/pkg/data_quality_inspector_bg.wasm?url&inline";
import { initSync, inspect } from "../wasm/pkg/data_quality_inspector.js";
import { ENTITIES, NOW, ROWS_BY_TYPE, SCHEMA } from "./fixtures/quality";
import { parseAnswer } from "./inspect";
import type { InspectInput } from "./quality";
import { toInput } from "./quality";

const base64 = wasm.slice(wasm.indexOf(",") + 1);
initSync({ module: Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)) });

describe("wasm inspect on fixtures", () => {
  const input = toInput(NOW, SCHEMA, ROWS_BY_TYPE);
  const rawAnswer = inspect(JSON.stringify(input));
  const out = parseAnswer(rawAnswer);

  it("calculates entities count for each type matching the fixture rows", () => {
    expect(ENTITIES.length).toBeGreaterThan(0);
    expect(out.types.length).toBe(Object.keys(ROWS_BY_TYPE).length);
    for (const [tName, rows] of Object.entries(ROWS_BY_TYPE)) {
      const tOut = out.types.find((t) => t.type === tName);
      expect(tOut).toBeDefined();
      expect(tOut?.entities).toBe(rows.length);
    }
  });

  it("calculates completeness as a valid ratio between 0 and 1 for all types", () => {
    for (const t of out.types) {
      expect(t.completeness).toBeGreaterThanOrEqual(0);
      expect(t.completeness).toBeLessThanOrEqual(1);
      for (const attr of t.attributes) {
        expect(attr.completeness).toBeGreaterThanOrEqual(0);
        expect(attr.completeness).toBeLessThanOrEqual(1);
      }
    }
  });

  it("computes valid share correctly: one of two Events is valid, lower for failing types, null for Vehicle", () => {
    const eventType = out.types.find((t) => t.type === "Event");
    expect(eventType).toBeDefined();
    // ev-fail carries an attribute the model does not have (additionalProperties: false).
    expect(eventType?.valid).toBe(0.5);
    expect(eventType?.findings.map((f) => [f.attribute, f.rule])).toEqual([["extraProp", "unknown"]]);

    const bikeType = out.types.find((t) => t.type === "BikeHireDockingStation");
    expect(bikeType).toBeDefined();
    expect(bikeType?.valid).not.toBeNull();
    expect(bikeType!.valid!).toBeLessThan(1.0);
    expect(bikeType!.valid!).toBeGreaterThan(0);

    const districtType = out.types.find((t) => t.type === "AdministrativeArea");
    expect(districtType).toBeDefined();
    expect(districtType?.valid).not.toBeNull();
    expect(districtType!.valid!).toBeLessThan(1.0);

    const vehicleType = out.types.find((t) => t.type === "Vehicle");
    expect(vehicleType).toBeDefined();
    expect(vehicleType?.valid).toBeNull();
    expect(vehicleType?.findings).toHaveLength(0);
  });

  it("produces findings with expected rule names and attributes for each failure type", () => {
    const bikeType = out.types.find((t) => t.type === "BikeHireDockingStation");
    expect(bikeType).toBeDefined();

    // 1. negative availableBikeNumber -> minimum
    const minFinding = bikeType?.findings.find(
      (f) => f.attribute === "availableBikeNumber" && f.rule === "minimum",
    );
    expect(minFinding).toBeDefined();
    expect(minFinding?.detail).toBe("0");

    // 2. non-integer availableBikeNumber (1.5) -> type
    const typeFinding = bikeType?.findings.find(
      (f) => f.attribute === "availableBikeNumber" && f.rule === "type",
    );
    expect(typeFinding).toBeDefined();
    expect(typeFinding?.detail).toBe("integer");

    // 3. invalid dateModified date-time -> format
    const formatFinding = bikeType?.findings.find(
      (f) => f.attribute === "dateModified" && f.rule === "format",
    );
    expect(formatFinding).toBeDefined();
    expect(formatFinding?.detail).toBe("date-time");


    // 5. location without coordinates -> required
    const reqFinding = bikeType?.findings.find(
      (f) => f.attribute === "location" && f.rule === "required",
    );
    expect(reqFinding).toBeDefined();
    expect(reqFinding?.detail).toBe("coordinates");

    // 6. districtCode with invalid pattern -> pattern
    const districtType = out.types.find((t) => t.type === "AdministrativeArea");
    expect(districtType).toBeDefined();
    const patternFinding = districtType?.findings.find(
      (f) => f.attribute === "districtCode" && f.rule === "pattern",
    );
    expect(patternFinding).toBeDefined();
    expect(patternFinding?.detail).toBe("^[0-9]{1,10}$");
  });

  it("counts $ref properties in notChecked and never fails them", () => {
    const districtType = out.types.find((t) => t.type === "AdministrativeArea");
    expect(districtType).toBeDefined();

    const divisionAttr = districtType?.attributes.find((a) => a.name === "divisionLevel");
    expect(divisionAttr).toBeDefined();
    expect(divisionAttr?.notChecked).toBeGreaterThan(0);

    const refFinding = districtType?.findings.find((f) => f.attribute === "divisionLevel");
    expect(refFinding).toBeUndefined();
  });

  it("computes freshness field and ages for types with timestamp properties, null when none", () => {
    const bikeType = out.types.find((t) => t.type === "BikeHireDockingStation");
    expect(bikeType?.freshness).not.toBeNull();
    expect(bikeType?.freshness?.field).toBe("dateModified");
    expect(typeof bikeType?.freshness?.medianSeconds).toBe("number");
    expect(bikeType!.freshness!.medianSeconds).toBeGreaterThanOrEqual(0);
    expect(bikeType!.freshness!.maxSeconds).toBeGreaterThanOrEqual(bikeType!.freshness!.medianSeconds);
    expect(bikeType!.freshness!.olderThanDay).toBeGreaterThanOrEqual(0);
    expect(bikeType!.freshness!.olderThanDay).toBeLessThanOrEqual(1);

    const vehicleType = out.types.find((t) => t.type === "Vehicle");
    expect(vehicleType?.freshness).toBeNull();

    const districtType = out.types.find((t) => t.type === "AdministrativeArea");
    expect(districtType?.freshness).toBeNull();
  });

  it("returns error JSON object when given bad JSON input, causing parseAnswer to throw", () => {
    const badJson = "not a valid json string";
    const answer = inspect(badJson);
    const parsed = JSON.parse(answer) as { error?: string };
    expect(parsed.error).toBeDefined();
    expect(parsed.error).toContain("the data could not be read");
    expect(() => parseAnswer(answer)).toThrow("the data could not be read");
  });
});

describe("wasm inspect isolated cases", () => {
  it("caps findings at 200 while keeping findingsTotal accurate", () => {
    const rows = Array.from({ length: 250 }, (_, i) => ({
      id: `urn:ngsi-ld:Test:${i}`,
      type: "Test",
      count: "not-an-integer",
    }));
    const input: InspectInput = {
      now: 1_700_000_000,
      types: [
        {
          type: "Test",
          schema: {
            properties: { count: { type: "integer" } },
          },
          rows,
        },
      ],
    };

    const out = parseAnswer(inspect(JSON.stringify(input)));
    expect(out.types[0].findings).toHaveLength(200);
    expect(out.types[0].findingsTotal).toBe(250);
  });

  it("sorts types by lowest valid share first, placing no-schema types last", () => {
    const input: InspectInput = {
      now: 1_700_000_000,
      types: [
        {
          type: "Perfect",
          schema: { properties: { v: { type: "integer" } } },
          rows: [{ id: "p1", v: 10 }],
        },
        {
          type: "HalfValid",
          schema: { properties: { v: { type: "integer" } } },
          rows: [{ id: "h1", v: 10 }, { id: "h2", v: "wrong" }],
        },
        {
          type: "NoSchema",
          schema: null,
          rows: [{ id: "n1", v: 10 }],
        },
      ],
    };

    const out = parseAnswer(inspect(JSON.stringify(input)));
    expect(out.types[0].type).toBe("HalfValid");
    expect(out.types[1].type).toBe("Perfect");
    expect(out.types[2].type).toBe("NoSchema");
  });
});
