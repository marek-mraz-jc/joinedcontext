/**
 * T-3224: the output node's mapper. A record's fields onto the type's attributes, the Bloblang it
 * generates, reading that step back, the problems shown at each attribute, and the preview.
 */
import { describe, expect, it } from "vitest";
import {
  autoMatch,
  fieldPath,
  generate,
  HEADER,
  idFieldOf,
  preview,
  problemsOf,
  readBack,
  type Attribute,
  type MapperState,
} from "../src/pages/pipelines/outputMapper";

const ATTRIBUTES: Attribute[] = [
  { name: "dateObserved", kind: "Property", valueType: "string", format: "date-time", required: true },
  { name: "pm10", kind: "Property", valueType: "number", required: false, unit: { code: "GQ", ucum: "ug/m3" } },
  { name: "status", kind: "Property", valueType: "string", required: false, values: ["working", "closed"] },
  { name: "refDevice", kind: "Relationship", valueType: "string", required: false, relationship: { target: "Device", many: false } },
  { name: "name", kind: "LanguageProperty", valueType: "object", required: false },
  { name: "location", kind: "GeoProperty", valueType: "object", required: false },
];

const RECORDS = [
  { "Station ID": "BB 01", observed: "2026-10-07T08:00:00Z", PM10: "18.4", state: "working", lon: "19.15", lat: "48.74" },
  { "Station ID": "BB 02", observed: "2026-10-07T08:00:00Z", PM10: "", state: "closed", lon: "19.16", lat: "48.75" },
];

const STATE: MapperState = {
  type: "AirQualityObserved",
  idField: "Station ID",
  attributes: {
    dateObserved: { field: "observed" },
    pm10: { field: "PM10" },
    status: { field: "state" },
    location: { longitude: "lon", latitude: "lat" },
    name: { expression: '{ "sk": this."Station ID" }' },
  },
};

describe("the output mapper", () => {
  it("quotes a field that is not a plain name", () => {
    expect(fieldPath("pm10")).toBe("this.pm10");
    expect(fieldPath("Station ID")).toBe('this."Station ID"');
    expect(fieldPath('a"b')).toBe('this."a\\"b"');
  });

  it("generates a step that mints the id, wraps each attribute as its kind and coerces CSV text", () => {
    const step = generate(STATE, ATTRIBUTES, "ovzdusie");
    expect(step.startsWith(HEADER)).toBe(true);
    expect(step).toContain('root.id = "urn:ngsi-ld:%v:%v:%v:%v".format("AirQualityObserved", $domain, "ovzdusie", this."Station ID".string()');
    expect(step).toContain('root.type = "AirQualityObserved"');
    expect(step).toContain('if this.PM10 != null && this.PM10 != "" { this.PM10.number() } else { null }');
    expect(step).toContain('"unitCode": "GQ"');
    expect(step).toContain('"type": "GeoProperty", "value"');
    expect(step).toContain('"type": "LanguageProperty", "languageMap"');
    // A coded slot keeps its text; an unmapped attribute writes nothing.
    expect(step).toContain("let a2 = this.state\n");
    expect(step).not.toContain("root.refDevice");
  });

  it("reads back its own step and nothing edited by hand", () => {
    const step = generate(STATE, ATTRIBUTES, "ovzdusie");
    expect(readBack(step, ATTRIBUTES, "ovzdusie")).toEqual(STATE);
    expect(readBack(`${step}root.extra = 1\n`, ATTRIBUTES, "ovzdusie")).toBeUndefined();
    expect(readBack(step.replace("ovzdusie", "elsewhere"), ATTRIBUTES, "ovzdusie")).toBeUndefined();
    expect(readBack("root = this", ATTRIBUTES, "ovzdusie")).toBeUndefined();
    expect(readBack(`${HEADER}{not json\nroot = this\n`, ATTRIBUTES, "ovzdusie")).toBeUndefined();
    expect(readBack(undefined, ATTRIBUTES, "ovzdusie")).toBeUndefined();
  });

  it("matches fields by name, case, accents and separators aside, and keeps a choice already made", () => {
    const fields = ["Date_Observed", "PM10", "Stav", "id"];
    expect(autoMatch(ATTRIBUTES, fields)).toEqual({ dateObserved: { field: "Date_Observed" }, pm10: { field: "PM10" } });
    expect(autoMatch(ATTRIBUTES, fields, { pm10: { value: 0 } }).pm10).toEqual({ value: 0 });
    expect(idFieldOf(fields)).toBe("id");
    expect(idFieldOf(["Station ID", "name"])).toBe("Station ID");
    expect(idFieldOf(["name"])).toBeUndefined();
  });

  it("names a missing required attribute, a field the sample lacks and a value the type cannot read", () => {
    expect(problemsOf(STATE, ATTRIBUTES, RECORDS)).toEqual([]);
    const broken: MapperState = {
      type: "AirQualityObserved",
      attributes: { pm10: { field: "state" }, status: { field: "PM10" }, refDevice: { field: "device" } },
    };
    expect(problemsOf(broken, ATTRIBUTES, RECORDS)).toEqual([
      { attribute: "", kind: "noId" },
      { attribute: "dateObserved", kind: "required" },
      { attribute: "pm10", kind: "notNumber", field: "state", value: "working" },
      { attribute: "status", kind: "notValue", field: "PM10", value: "18.4", values: ["working", "closed"] },
      { attribute: "refDevice", kind: "notInSample", field: "device" },
    ]);
    // Without a sample there is nothing to compare a field with, and only the map is judged.
    expect(problemsOf(broken, ATTRIBUTES, []).map((p) => p.kind)).toEqual(["noId", "required"]);
  });

  it("previews up to three entities as NGSI-LD, leaving out what a record does not have", () => {
    const [first, second] = preview(STATE, ATTRIBUTES, RECORDS, "ovzdusie", "banskabystrica.sk");
    expect(first).toEqual({
      id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:BB-01",
      type: "AirQualityObserved",
      dateObserved: { type: "Property", value: "2026-10-07T08:00:00Z" },
      pm10: { type: "Property", value: 18.4, unitCode: "GQ" },
      status: { type: "Property", value: "working" },
      name: { type: "LanguageProperty", languageMap: '({ "sk": this."Station ID" })' },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [19.15, 48.74] } },
    });
    expect(second).not.toHaveProperty("pm10");
    expect(preview(STATE, ATTRIBUTES, [...RECORDS, ...RECORDS], "ovzdusie", "x")).toHaveLength(3);
  });
});
