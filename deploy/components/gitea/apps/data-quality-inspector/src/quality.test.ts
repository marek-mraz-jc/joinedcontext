import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import type { TypeQuality } from "./quality";
import {
  KNOWN_TYPES,
  formatAge,
  formatRule,
  localId,
  percent,
  readHash,
  readView,
  resolveTypes,
  sortTypesByQuality,
  toInput,
  writeHash,
  writeView,
} from "./quality";

describe("toInput", () => {
  it("converts Map and Schema into inspector input shape", () => {
    const rows: Row[] = [
      { id: "urn:ngsi-ld:Event:1", type: "Event", name: "Music Festival" },
      { id: "urn:ngsi-ld:Event:2", type: "Event", name: "Art Fair" },
    ];
    const map = new Map<string, Row[]>([["Event", rows]]);
    const schema = {
      Event: {
        properties: { name: { type: "string" } },
      },
    };

    const input = toInput(1_700_000_000, schema, map);
    expect(input.now).toBe(1_700_000_000);
    expect(input.types).toHaveLength(1);
    expect(input.types[0].type).toBe("Event");
    expect(input.types[0].schema).toEqual({ properties: { name: { type: "string" } } });
    expect(input.types[0].rows).toHaveLength(2);
  });

  it("converts Record and null Schema into inspector input shape with schema null", () => {
    const record = {
      Vehicle: [{ id: "urn:ngsi-ld:Vehicle:1", type: "Vehicle", speed: 50 }],
    };
    const input = toInput(1_700_000_000, null, record);
    expect(input.now).toBe(1_700_000_000);
    expect(input.types).toHaveLength(1);
    expect(input.types[0].type).toBe("Vehicle");
    expect(input.types[0].schema).toBeNull();
    expect(input.types[0].rows).toHaveLength(1);
  });
});

describe("formatAge", () => {
  it("formats seconds into human words in Finnish and English", () => {
    // Under a minute
    expect(formatAge(30, "en")).toBe("30 s");
    expect(formatAge(30, "fi")).toBe("30 s");

    // Under an hour
    expect(formatAge(180, "en")).toBe("3 min");
    expect(formatAge(180, "fi")).toBe("3 min");

    // Under a day
    expect(formatAge(7200, "en")).toBe("2 h");
    expect(formatAge(7200, "fi")).toBe("2 h");

    // One day (singular in English)
    expect(formatAge(86400, "en")).toBe("1 day");
    expect(formatAge(86400, "fi")).toBe("1 pv");

    // Multiple days
    expect(formatAge(172800, "en")).toBe("2 days");
    expect(formatAge(172800, "fi")).toBe("2 pv");

    // Zero or negative or non-finite
    expect(formatAge(-10, "en")).toBe("0 min");
    expect(formatAge(Number.NaN, "fi")).toBe("0 min");
  });
});

describe("formatRule", () => {
  it("formats validation failure rules in Finnish and English", () => {
    // Format date-time, date, uri, other
    expect(formatRule("format", "date-time", "en")).toBe("not a date-time");
    expect(formatRule("format", "date-time", "fi")).toBe("ei kelvollinen aikaleima");

    expect(formatRule("format", "date", "en")).toBe("not a date");
    expect(formatRule("format", "date", "fi")).toBe("ei kelvollinen päivämäärä");

    expect(formatRule("format", "uri", "en")).toBe("not a URI");
    expect(formatRule("format", "uri", "fi")).toBe("ei kelvollinen URI");

    expect(formatRule("format", "email", "en")).toBe("not a valid email");
    expect(formatRule("format", "email", "fi")).toBe("ei kelvollinen email");

    // Minimum and maximum
    expect(formatRule("minimum", "0", "en")).toBe("below the minimum 0");
    expect(formatRule("minimum", "0", "fi")).toBe("alle minimin 0");

    expect(formatRule("maximum", "100", "en")).toBe("above the maximum 100");
    expect(formatRule("maximum", "100", "fi")).toBe("yli maksimin 100");

    // Enum
    expect(formatRule("enum", "active, inactive", "en")).toBe("not one of: active, inactive");
    expect(formatRule("enum", "active, inactive", "fi")).toBe("ei mikään näistä: active, inactive");

    // Required and unknown
    expect(formatRule("required", "location", "en")).toBe("missing, but required");
    expect(formatRule("required", "location", "fi")).toBe("puuttuu, mutta pakollinen");

    expect(formatRule("unknown", "extra", "en")).toBe("not in the model");
    expect(formatRule("unknown", "extra", "fi")).toBe("ei mallissa");

    // Pattern and type
    expect(formatRule("pattern", "^[0-9]+$", "en")).toBe("does not match pattern ^[0-9]+$");
    expect(formatRule("pattern", "^[0-9]+$", "fi")).toBe("ei vastaa säännöllistä lauseketta ^[0-9]+$");

    expect(formatRule("type", "integer", "en")).toBe("not of type integer");
    expect(formatRule("type", "integer", "fi")).toBe("ei tyyppiä integer");

    // Fallback for custom rule
    expect(formatRule("customRule", "detail-msg", "en")).toBe("customRule: detail-msg");
    expect(formatRule("customRule", "", "en")).toBe("customRule");
  });
});

describe("hash navigation state", () => {
  it("round trips type in hash view state", () => {
    const hash = writeHash("BikeHireDockingStation");
    expect(hash).toBe("#quality?type=BikeHireDockingStation");
    expect(readHash(hash)).toEqual({ type: "BikeHireDockingStation" });

    const nullHash = writeHash(null);
    expect(nullHash).toBe("#quality");
    expect(readHash(nullHash)).toEqual({ type: null });

    expect(readView).toBe(readHash);
    expect(writeView).toBe(writeHash);
  });

  it("handles URL encoded characters in type parameter", () => {
    const hash = writeHash("Public Area Permit");
    expect(hash).toBe("#quality?type=Public%20Area%20Permit");
    expect(readHash(hash)).toEqual({ type: "Public Area Permit" });
  });

  it("ignores junk and malformed hash parameters", () => {
    expect(readHash("")).toEqual({ type: null });
    expect(readHash("#")).toEqual({ type: null });
    expect(readHash("#quality")).toEqual({ type: null });
    expect(readHash("#quality?type=")).toEqual({ type: null });
    expect(readHash("#quality?type=   ")).toEqual({ type: null });
    expect(readHash("#other?foo=bar")).toEqual({ type: null });
    expect(readHash("?type=Event&extra=123")).toEqual({ type: "Event" });
  });
});

describe("localId", () => {
  it("extracts local id part after the last colon, handling URNs and bare IDs", () => {
    expect(localId("urn:ngsi-ld:BikeHireDockingStation:station-01")).toBe("station-01");
    expect(localId("urn:ngsi-ld:Alert:hel.fi:helsinki:GUID-42")).toBe("GUID-42");
    expect(localId("plain-id")).toBe("plain-id");
    expect(localId("")).toBe("");
  });
});

describe("percent", () => {
  it("formats numbers as rounded percentages and null/undefined/NaN as dash", () => {
    expect(percent(0.856, "en")).toBe("86 %");
    expect(percent(1, "fi")).toBe("100 %");
    expect(percent(0, "en")).toBe("0 %");
    expect(percent(null, "en")).toBe("—");
    expect(percent(undefined, "fi")).toBe("—");
    expect(percent(Number.NaN, "en")).toBe("—");
    expect(percent(Number.POSITIVE_INFINITY, "en")).toBe("—");
  });
});

describe("resolveTypes", () => {
  it("falls back to KNOWN_TYPES when schema is missing or empty", () => {
    expect(resolveTypes(null)).toEqual([...KNOWN_TYPES]);
    expect(resolveTypes({})).toEqual([...KNOWN_TYPES]);
  });

  it("returns sorted type names from published schema", () => {
    const schema = {
      Event: {},
      AirQualityObserved: {},
      BikeHireDockingStation: {},
    };
    expect(resolveTypes(schema)).toEqual(["AirQualityObserved", "BikeHireDockingStation", "Event"]);
  });
});

describe("sortTypesByQuality", () => {
  it("sorts worst valid first, placing types without schema (null) at the end", () => {
    const tBad: TypeQuality = {
      type: "BadType",
      entities: 10,
      completeness: 0.8,
      valid: 0.4,
      attributes: [],
      findings: [],
      findingsTotal: 6,
      freshness: null,
    };
    const tGood: TypeQuality = {
      type: "GoodType",
      entities: 10,
      completeness: 0.9,
      valid: 1.0,
      attributes: [],
      findings: [],
      findingsTotal: 0,
      freshness: null,
    };
    const tNoSchema: TypeQuality = {
      type: "NoSchemaType",
      entities: 5,
      completeness: 0.7,
      valid: null,
      attributes: [],
      findings: [],
      findingsTotal: 0,
      freshness: null,
    };
    const tTiedBad: TypeQuality = {
      type: "AnotherBad",
      entities: 10,
      completeness: 0.6,
      valid: 0.4,
      attributes: [],
      findings: [],
      findingsTotal: 6,
      freshness: null,
    };

    const sorted = sortTypesByQuality([tGood, tNoSchema, tBad, tTiedBad]);
    expect(sorted.map((t) => t.type)).toEqual(["AnotherBad", "BadType", "GoodType", "NoSchemaType"]);
  });
});
