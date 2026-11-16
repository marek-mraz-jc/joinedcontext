/**
 * T-7062 (G-18): the mapper's sources past a plain field. Each new kind and transform generates
 * fixed Bloblang, reads back to the same state, and previews the entity the runner writes; the
 * expected output of the generated step is ekiben's own (evidence/T-7062/mapper-g18-out.jsonl).
 */
import { describe, expect, it } from "vitest";
import { generate, preview, problemsOf, readBack, shapedValue, type Attribute, type MapperState, type Source } from "../src/pages/pipelines/outputMapper";

const ATTRIBUTES: Attribute[] = [
  { name: "name", kind: "LanguageProperty", required: false },
  { name: "refDistrict", kind: "Relationship", required: false, relationship: { target: "District", many: false } },
  { name: "location", kind: "GeoProperty", required: false },
  { name: "shape", kind: "GeoProperty", required: false },
  { name: "height", kind: "Property", valueType: "number", required: false, unit: { code: "MTR" } },
  { name: "status", kind: "Property", valueType: "string", required: false, values: ["working", "broken"] },
  { name: "planted", kind: "Property", valueType: "string", format: "date", required: false },
  { name: "label", kind: "Property", valueType: "string", required: false },
];

const RECORD = {
  ID: "T 1",
  NOM_CA: "Plàtan",
  NOM_ES: "",
  "CODI DISTRICTE": "01",
  X: "430887.56",
  Y: "4581837.85",
  WKT: "POINT (2.17 41.38)",
  GEO: '{"type":"Point","coordinates":[2.17,41.38]}',
  ALCADA: "1.234,5",
  MESURAT: "2026-10-20T14:30:00Z",
  ESTAT: "  Bo ",
  DATA: "20/10/2025",
  ADRECA: "C/ Major 1",
};

const SOURCES: Record<string, [string, Source]> = {
  ref: ["refDistrict", { ref: { type: "District", field: "CODI DISTRICTE" } }],
  languages: ["name", { languages: { ca: "NOM_CA", es: "NOM_ES" } }],
  point: ["location", { longitude: "X", latitude: "Y", crs: "EPSG:25831" }],
  wkt: ["shape", { geometry: "WKT", format: "wkt" }],
  geojson: ["shape", { geometry: "GEO", format: "geojson" }],
  decimalComma: ["height", { field: "ALCADA", transforms: [{ op: "decimalComma" }, { op: "default", value: "0" }] }],
  lookup: ["status", { field: "ESTAT", transforms: [{ op: "trim" }, { op: "lower" }, { op: "lookup", table: { bo: "working", dolent: "broken" } }] }],
  date: ["planted", { field: "DATA", transforms: [{ op: "date", format: "%d/%m/%Y" }] }],
  concat: ["label", { concat: ["NOM_CA", "ADRECA"], separator: " - " }],
};

const one = (attribute: string, source: Source, extra: Partial<MapperState> = {}): MapperState => ({
  type: "Tree",
  idField: "ID",
  attributes: { [attribute]: source },
  ...extra,
});

describe("the mapper's G-18 sources", () => {
  it.each(Object.entries(SOURCES))("reads back its own %s step to the same state", (_, [attribute, source]) => {
    const state = one(attribute, source, { observedAt: attribute === "height" ? { height: "MESURAT" } : undefined });
    const step = generate(state, ATTRIBUTES, "trees");
    expect(readBack(step, ATTRIBUTES, "trees")).toEqual(state);
    // A step edited by hand is the author's: one character off and the mapper lets it be.
    expect(readBack(step.replace("root.type", "root.kind"), ATTRIBUTES, "trees")).toBeUndefined();
  });

  it("writes each kind through fixed templates, field names quoted and nothing typed as code", () => {
    const hostile = 'x" } root = deleted() # ';
    const step = generate(
      one("label", { field: hostile, transforms: [{ op: "default", value: '"); root = deleted() #' }] }),
      ATTRIBUTES,
      "trees",
    );
    expect(step).toContain(`this.${JSON.stringify(hostile)}`);
    expect(step).toContain(JSON.stringify('"); root = deleted() #'));
    expect(step.split("\n").filter((line) => line.startsWith("root = deleted()"))).toEqual([]);
    const ref = generate(one(...SOURCES.ref), ATTRIBUTES, "trees");
    expect(ref).toContain('"urn:ngsi-ld:%v:%v:%v:%v".format("District", $owner, "trees", this."CODI DISTRICTE".string()');
    expect(generate(one(...SOURCES.point), ATTRIBUTES, "trees")).toContain('.geo_transform("EPSG:25831", "EPSG:4326")');
    expect(generate(one(...SOURCES.wkt), ATTRIBUTES, "trees")).toContain("this.WKT.string().parse_wkt()");
    expect(generate(one(...SOURCES.lookup), ATTRIBUTES, "trees")).toContain('match this.ESTAT.string().trim().string().lowercase().string() { "bo" => "working", "dolent" => "broken", _ => this }');
  });

  it("leaves a plain field's step exactly as before, so every existing pipeline still reads back", () => {
    const step = generate(one("height", { field: "ALCADA" }), ATTRIBUTES, "trees");
    expect(step).toContain("let a4 = if this.ALCADA != null && this.ALCADA != \"\" { this.ALCADA.number() } else { null }");
    expect(step).toContain('root.height = if $a4 != null { { "type": "Property", "value": $a4, "unitCode": "MTR" } } else { deleted() }');
  });

  it("previews what the runner writes for the Barcelona record", () => {
    const state: MapperState = {
      type: "Tree",
      idField: "ID",
      attributes: Object.fromEntries(Object.values(SOURCES).filter(([, source]) => !("format" in source && source.format === "geojson")).map(([a, s]) => [a, s])),
      observedAt: { height: "MESURAT" },
    };
    const [entity] = preview(state, ATTRIBUTES, [RECORD], "trees", "acme");
    // As ekiben wrote it (evidence/T-7062/mapper-g18-out.jsonl); the reprojection and WKT are the runner's.
    expect(entity.refDistrict).toEqual({ type: "Relationship", object: "urn:ngsi-ld:District:acme:trees:01" });
    expect(entity.name).toEqual({ type: "LanguageProperty", languageMap: { ca: "Plàtan" } });
    expect(entity.height).toEqual({ type: "Property", value: 1234.5, unitCode: "MTR", observedAt: "2026-10-20T14:30:00Z" });
    expect(entity.status).toEqual({ type: "Property", value: "working" });
    expect(entity.planted).toEqual({ type: "Property", value: "2025-10-20" });
    expect(entity.label).toEqual({ type: "Property", value: "Plàtan - C/ Major 1" });
    const [empty] = preview(state, ATTRIBUTES, [{ ID: "T2", ESTAT: "raro", ALCADA: "" }], "trees", "acme");
    expect(empty.height).toEqual({ type: "Property", value: 0, unitCode: "MTR" });
    expect(empty.status).toEqual({ type: "Property", value: "raro" });
    expect(empty.name).toBeUndefined();
    expect(empty.label).toBeUndefined();
  });

  it("names a field the sample lacks, a date the format cannot read and an observedAt that is no time", () => {
    const records = [{ ...RECORD, DATA: "2025-10-20", MESURAT: "yesterday" }];
    const problems = problemsOf(
      { type: "Tree", idField: "ID", attributes: { name: { languages: { ca: "NOM_CA", es: "NOM_FR" } }, planted: SOURCES.date[1], height: { field: "ALCADA" } }, observedAt: { height: "MESURAT" } },
      ATTRIBUTES,
      records,
    );
    expect(problems).toContainEqual({ attribute: "name", kind: "notInSample", field: "NOM_FR" });
    expect(problems).toContainEqual({ attribute: "planted", kind: "notShaped", field: "DATA", value: "2025-10-20" });
    expect(problems).toContainEqual({ attribute: "height", kind: "notTime", field: "MESURAT", value: "yesterday" });
    // The enum check reads the translated value: " Bo " becomes working, one of the model's.
    expect(problemsOf(one(...SOURCES.lookup), ATTRIBUTES, [RECORD])).toEqual([]);
    expect(shapedValue("31.02.2025", [{ op: "date", format: "%d.%m.%Y" }], ATTRIBUTES[6])).toBeUndefined();
  });
});
