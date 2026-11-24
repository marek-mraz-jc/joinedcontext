/**
 * T-7680: the output mapper's less travelled branches. Each source kind and transform as the step
 * writes it and as the preview shows it, the fields a source reads, a header that parses but is not
 * a map, and the checks for a boolean slot and a field named by an empty name.
 */
import { describe, expect, it } from "vitest";
import {
  fieldsOf,
  generate,
  HEADER,
  preview,
  problemsOf,
  readBack,
  shapedValue,
  type Attribute,
  type MapperState,
} from "../src/pages/pipelines/outputMapper";
import { withEntityEnd } from "../src/pages/pipelines/OutputMapper";
import { sampleValues } from "../src/pages/pipelines/MapperSources";

const text = (name: string): Attribute => ({ name, kind: "Property", valueType: "string", required: false });
const typed = (name: string, valueType: Attribute["valueType"]): Attribute => ({ name, kind: "Property", valueType, required: false });

describe("the mapper's step", () => {
  it("writes an upper-case transform and a typed boolean default", () => {
    const step = generate(
      {
        type: "Sensor",
        attributes: {
          code: { field: "c", transforms: [{ op: "upper" }] },
          active: { field: "a", transforms: [{ op: "trim" }, { op: "default", value: "true" }] },
          note: { field: "n", transforms: [{ op: "default", value: "yes" }] },
        },
      },
      [text("code"), typed("active", "boolean"), typed("note", "boolean")],
      "air",
    );
    expect(step).toContain("this.c.string().uppercase()");
    expect(step).toContain("this.a.string().trim().bool() } else { true }");
    // A default the boolean slot cannot read stays text rather than becoming a wrong literal.
    expect(step).toContain('} else { "yes" }');
  });

  it("writes a fixed value, and null for a language or concat source with no fields", () => {
    const step = generate(
      { type: "Sensor", attributes: { label: { value: "fixed" }, name: { languages: {} }, both: { concat: [], separator: " " } } },
      [text("label"), { name: "name", kind: "LanguageProperty", valueType: "object", required: false }, text("both")],
      "air",
    );
    expect(step).toContain('let a0 = "fixed"');
    expect(step).toContain("let a1 = null");
    expect(step).toContain("let a2 = null");
  });

  it("reads back nothing when the header holds JSON that is not a map", () => {
    expect(readBack(`${HEADER}{"type":1,"attributes":{}}\n`, [], "air")).toBeUndefined();
    expect(readBack(`${HEADER}{"type":"T","attributes":null}\n`, [], "air")).toBeUndefined();
    expect(readBack(`${HEADER}null\n`, [], "air")).toBeUndefined();
  });
});

describe("the fields a source reads", () => {
  it("names each field of every source kind, and none for a value or an expression", () => {
    expect(fieldsOf({ ref: { type: "Device", field: "dev" } })).toEqual(["dev"]);
    expect(fieldsOf({ concat: ["a", "b"], separator: "-" })).toEqual(["a", "b"]);
    expect(fieldsOf({ languages: { en: "nameEn", sk: "nameSk" } })).toEqual(["nameEn", "nameSk"]);
    expect(fieldsOf({ geometry: "geom", format: "wkt" })).toEqual(["geom"]);
    expect(fieldsOf({ longitude: "x", latitude: "y" })).toEqual(["x", "y"]);
    expect(fieldsOf({ value: 3 })).toEqual([]);
    expect(fieldsOf({ expression: "this.a" })).toEqual([]);
  });
});

describe("a transformed value in the sample", () => {
  it("upper-cases text and leaves an empty value alone", () => {
    expect(shapedValue("abc", [{ op: "upper" }], text("x"))).toBe("ABC");
    expect(shapedValue("", [{ op: "upper" }], text("x"))).toBe("");
    expect(shapedValue(12, [{ op: "upper" }], text("x"))).toBe("12");
  });
});

describe("the mapper's problems", () => {
  it("names a boolean slot whose sample value is neither true nor false", () => {
    const state: MapperState = { type: "T", idField: "id", attributes: { active: { field: "on" } } };
    expect(problemsOf(state, [typed("active", "boolean")], [{ id: "1", on: "maybe" }])).toEqual([
      { attribute: "active", kind: "notBoolean", field: "on", value: "maybe" },
    ]);
    expect(problemsOf(state, [typed("active", "boolean")], [{ id: "1", on: true }])).toEqual([]);
  });

  it("names a field with an empty name the sample cannot hold", () => {
    const state: MapperState = { type: "T", idField: "id", attributes: { label: { field: "" } } };
    expect(problemsOf(state, [text("label")], [{ id: "1" }])).toEqual([{ attribute: "label", kind: "notInSample", field: "" }]);
  });

  it("reads a Unix-seconds observedAt as a time and refuses a text that is not one", () => {
    const state: MapperState = { type: "T", idField: "id", attributes: { label: { field: "l" } }, observedAt: { label: "at" } };
    expect(problemsOf(state, [text("label")], [{ id: "1", l: "x", at: 1_760_000_000.7 }])).toEqual([]);
    expect(problemsOf(state, [text("label")], [{ id: "1", l: "x", at: "yesterday" }])).toEqual([
      { attribute: "label", kind: "notTime", field: "at", value: "yesterday" },
    ]);
  });
});

describe("the mapper's preview", () => {
  const record = { id: "a 1", n: "4.6", on: "true", geo: '{"type":"Point","coordinates":[17.1,48.1]}', bad: "{not json", at: 1_760_000_000 };

  it("rounds an integer, reads a boolean, keeps a fixed value and observedAt from Unix seconds", () => {
    const state: MapperState = {
      type: "T",
      idField: "id",
      attributes: { count: { field: "n" }, active: { field: "on" }, label: { value: "fixed" } },
      observedAt: { count: "at" },
    };
    const [entity] = preview(state, [typed("count", "integer"), typed("active", "boolean"), text("label")], [record], "air", "city.example");
    expect(entity).toEqual({
      id: "urn:ngsi-ld:T:city.example:air:a-1",
      type: "T",
      count: { type: "Property", value: 5, observedAt: "2025-10-09T08:53:20Z" },
      active: { type: "Property", value: true },
      label: { type: "Property", value: "fixed" },
    });
  });

  it("parses a GeoJSON field, keeps text that is not JSON as it is, and takes an object as it is", () => {
    const geo: Attribute = { name: "location", kind: "GeoProperty", valueType: "object", required: false };
    const point = { type: "Point", coordinates: [17.1, 48.1] };
    const of = (field: string, row: Record<string, unknown>) =>
      preview({ type: "T", attributes: { location: { geometry: field, format: "geojson" } } }, [geo], [row], "air", "city.example")[0].location;
    expect(of("geo", record)).toEqual({ type: "GeoProperty", value: point });
    expect(of("bad", record)).toEqual({ type: "GeoProperty", value: "{not json" });
    expect(of("g", { g: point })).toEqual({ type: "GeoProperty", value: point });
  });

  it("writes no expiresAt when the lifetime's fixed time is not a time", () => {
    const [entity] = preview({ type: "T", attributes: {} }, [], [{}], "air", "city.example", 0, { at: "soon" });
    expect(entity).toEqual({ type: "T" });
  });
});

describe("the entity end of a type in the lifetime", () => {
  it("sets, replaces and removes a type's entity end and keeps its attribute ends", () => {
    const attributes = [{ names: ["pm10"], rolling: "PT1H" }];
    expect(withEntityEnd(undefined, "T", { rolling: "PT1M" })).toEqual([{ type: "T", entity: { rolling: "PT1M" } }]);
    expect(withEntityEnd(undefined, "T", undefined)).toBeUndefined();
    const other = { type: "U", entity: { rolling: "P1D" } };
    expect(withEntityEnd([other, { type: "T", attributes }], "T", { from: "dateObserved" })).toEqual([
      other,
      { type: "T", attributes, entity: { from: "dateObserved" } },
    ]);
    expect(withEntityEnd([other, { type: "T", attributes, entity: { rolling: "PT1M" } }], "T", undefined)).toEqual([other, { type: "T", attributes }]);
    expect(withEntityEnd([{ type: "T", entity: { rolling: "PT1M" } }], "T", undefined)).toBeUndefined();
  });
});

describe("the sample values a lookup lists", () => {
  it("lists each value once, as text, skipping empty ones, up to twenty", () => {
    const records = Array.from({ length: 25 }, (_, n) => ({ code: `c${n}` }));
    expect(sampleValues(records, "code")).toHaveLength(20);
    expect(sampleValues([{ code: 1 }, { code: 1 }, { code: "" }, { code: null }, {}, { code: { a: 1 } }], "code")).toEqual(["1", '{"a":1}']);
  });
});
