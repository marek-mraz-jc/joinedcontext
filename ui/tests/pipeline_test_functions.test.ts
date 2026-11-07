/**
 * What a dropped sample becomes before anybody sees it (T-1135, PL-43, PL-44).
 *
 * These are the functions the test form runs on a file the moment it lands: what format it is,
 * what type it describes, which column is the id, and the Bloblang that maps it. A bug in any of
 * them reaches a person as a mapping that does not compile, so the cases here are the ones a
 * real municipal export actually has — a semicolon delimiter, a Unicode header, a column called
 * `station_id`, a number written as text, an empty file.
 */
import { describe, expect, it } from "vitest";
import {
  sourceBodyOf,
  testFormatOf,
  draftFromRecord,
  draftFromSample,
  errorLines,
  firstRecordOf,
  formatOf,
  idColumnOf,
  literalTypeOf,
  spaceOfTarget,
  typeOf,
  withSample,
} from "../src/pages/pipelines/PipelineTest";

describe("the format a sample is in", () => {
  it("takes the file name's word first", () => {
    expect(formatOf("sensors.csv", "{not json at all")).toBe("csv");
    expect(formatOf("sensors.json", "a,b,c")).toBe("json");
    expect(formatOf("SENSORS.CSV", "")).toBe("csv");
  });

  it("reads the first bytes when the name says nothing", () => {
    expect(formatOf("sample", '{"id": 1}')).toBe("json");
    expect(formatOf("sample", "  [1, 2]")).toBe("json");
    expect(formatOf("sample", "id,name\n1,a")).toBe("csv");
    expect(formatOf("sample", "one line, no structure at all")).toBe("csv");
    expect(formatOf("sample", "just text")).toBe("text");
    expect(formatOf("sample", "")).toBe("text");
  });
});

describe("the entity type a file name describes", () => {
  it("is the stem in CamelCase", () => {
    expect(typeOf("air_quality-2026.csv")).toBe("AirQuality2026");
    expect(typeOf("events.json")).toBe("Events");
    expect(typeOf("sample")).toBe("Sample");
  });

  it("never starts with a digit, because an NGSI-LD type may not", () => {
    expect(typeOf("123abc.csv")).toBe("Sample123abc");
    expect(typeOf("2026.csv")).toBe("Sample2026");
    // Nothing left of the name at all is still a type.
    expect(typeOf("---.csv")).toBe("Sample");
  });
});

describe("the column a local id comes from (PL-44)", () => {
  it("prefers a column that says it is one", () => {
    expect(idColumnOf(["id", "name"])).toBe("id");
    expect(idColumnOf(["name", "identifier"])).toBe("identifier");
    expect(idColumnOf(["name", "ID"])).toBe("ID");
  });

  it("then one that ends in an id", () => {
    expect(idColumnOf(["station_id", "name"])).toBe("station_id");
    expect(idColumnOf(["name", "stationId"])).toBe("stationId");
  });

  it("and falls back to the first column, or to `id` when there is none", () => {
    expect(idColumnOf(["x", "y"])).toBe("x");
    expect(idColumnOf([])).toBe("id");
  });
});

describe("the first record of a sample", () => {
  it("pairs a CSV header with the first row", () => {
    expect(firstRecordOf("id,name\n1,Kallio", "csv")).toEqual({ id: "1", name: "Kallio" });
  });

  it("takes a semicolon as the delimiter when there is no comma", () => {
    expect(firstRecordOf("id;name\n1;Kallio", "csv")).toEqual({ id: "1", name: "Kallio" });
  });

  it("unquotes a cell and keeps a Unicode header as it is", () => {
    expect(firstRecordOf('id,"mesto"\n1,"Banská Bystrica"', "csv")).toEqual({
      id: "1",
      mesto: "Banská Bystrica",
    });
  });

  it("gives a header with no row its empty values, and an empty file nothing", () => {
    expect(firstRecordOf("id,name", "csv")).toEqual({ id: "", name: "" });
    expect(firstRecordOf("", "csv")).toBeNull();
    expect(firstRecordOf("\n\n", "csv")).toBeNull();
  });

  it("takes the first element of a JSON array, or the object itself", () => {
    expect(firstRecordOf('[{"id":1},{"id":2}]', "json")).toEqual({ id: 1 });
    expect(firstRecordOf('{"id":1}', "json")).toEqual({ id: 1 });
  });

  it("answers nothing for JSON that is not a record, and for text", () => {
    expect(firstRecordOf("[1,2,3]", "json")).toBeNull();
    expect(firstRecordOf("{not json", "json")).toBeNull();
    expect(firstRecordOf("[]", "json")).toBeNull();
    expect(firstRecordOf("id,name\n1,a", "text")).toBeNull();
  });
});

describe("the mapping a sample drafts (PL-44)", () => {
  const draft = draftFromSample("air_quality.csv", "station_id,pm10,mesto\nBB1,21.5,Banská Bystrica", "ovzdusie");

  it("mints the id under the organization's domain and the target space", () => {
    expect(draft).not.toBeNull();
    expect(draft?.bloblang).toContain('let domain = env("JC_ORG_DOMAIN")');
    expect(draft?.bloblang).toContain('"urn:ngsi-ld:%v:%v:%v:%v".format("AirQuality", $domain, "ovzdusie", this.station_id.string())');
    expect(draft?.type).toBe("AirQuality");
    expect(draft?.idColumn).toBe("station_id");
  });

  it("writes a number as a number and everything else as it came", () => {
    expect(draft?.bloblang).toContain('root.pm10 = { "type": "Property", "value": this.pm10.number().catch(this.pm10) }');
    expect(draft?.bloblang).toContain('root.mesto = { "type": "Property", "value": this.mesto }');
  });

  it("leaves the id column out of the attributes, because it is the id", () => {
    expect(draft?.bloblang).not.toContain("root.station_id =");
  });

  it("quotes a column name Bloblang cannot address bare, and names the attribute legally", () => {
    const awkward = draftFromSample("x.csv", 'id,pm 10 (µg/m³)\n1,21', "ovzdusie");
    // Bloblang's own quoted-path form, `this."name"`, and an attribute name made of the
    // characters a path may hold — the µ and the ³ are not among them.
    expect(awkward?.bloblang).toContain('this."pm 10 (µg/m³)"');
    expect(awkward?.bloblang).toContain("root.pm_10___g_m__ =");
  });

  // T-3211: a record a picked source answered drafts the same way, typed as the target model asks.
  it("drafts from one record of a source, typed as asked, its own `type` and `@context` left out", () => {
    const drafted = draftFromRecord(
      "shmu-csv",
      { id: "BB1", type: "Station", "@context": "x", pm10: 21.5 },
      "ovzdusie",
      "json",
      "AirQualityObserved",
    );
    expect(drafted?.bloblang).toContain('root.type = "AirQualityObserved"');
    expect(drafted?.bloblang).toContain('.format("AirQualityObserved", $domain, "ovzdusie", this.id.string())');
    expect(drafted?.bloblang).not.toContain("root.type = { ");
    expect(drafted?.bloblang).not.toContain("@context");
    expect(drafted?.columns).toEqual(["id", "type", "pm10"]);
    expect(draftFromRecord("shmu-csv", { "@context": "x" }, "ovzdusie")).toBeNull();
    expect(draftFromRecord("shmu-csv", { pm10: 1 }, "ovzdusie")?.type).toBe("ShmuCsv");
  });

  it("keeps two columns that fold to one name apart (F-R2UI4-016)", () => {
    const bloblang = draftFromRecord("x", { id: "1", "a-b": "first", "a b": "second" }, "s")?.bloblang ?? "";
    expect(bloblang).toContain('root.a_b = { "type": "Property", "value": this."a-b" }');
    expect(bloblang).toContain('root.a_b_2 = { "type": "Property", "value": this."a b" }');
  });

  it("drafts nothing from a file with no record or no column", () => {
    expect(draftFromSample("empty.csv", "", "ovzdusie")).toBeNull();
    expect(draftFromSample("notes.txt", "a line of prose", "ovzdusie")).toBeNull();
  });
});

describe("the space a target endpoint names", () => {
  it("is the fifth segment of the endpoint's URN", () => {
    expect(spaceOfTarget("urn:ngsi-ld:Endpoint:hel.fi:air-quality:ep-all", "helsinki")).toBe("air-quality");
  });

  it("falls back when there is no target, or it is not an Endpoint URN", () => {
    expect(spaceOfTarget(undefined, "helsinki")).toBe("helsinki");
    expect(spaceOfTarget("urn:ngsi-ld:ContextSpace:hel.fi:air-quality:x", "helsinki")).toBe("helsinki");
    expect(spaceOfTarget("urn:ngsi-ld:Endpoint:hel.fi::ep", "helsinki")).toBe("helsinki");
    expect(spaceOfTarget("not-a-urn", "helsinki")).toBe("helsinki");
  });
});

describe("the mapping lines an error names", () => {
  it("is the set of the lines, without the ones that name none", () => {
    expect(
      errorLines({
        errors: [{ line: 3, message: "x" }, { line: 7, message: "y" }, { line: 3, message: "z" }],
      } as never),
    ).toEqual(new Set([3, 7]));
  });

  it("ignores line 0 and a missing line, which mean the error is not on one", () => {
    expect(errorLines({ errors: [{ line: 0, message: "x" }, { message: "y" }] } as never)).toEqual(new Set());
    expect(errorLines({ errors: [] } as never)).toEqual(new Set());
  });
});

describe("a sample dropped on a pipeline that already has a mapping (T-3971)", () => {
  const drafted = draftFromSample("hfp.json", '{"VP": {"veh": 1}}', "helsinki");
  if (!drafted) throw new Error("hfp.json drafts a mapping");

  it("keeps a typed mapping and takes the type the mapping writes, not the file's name", () => {
    const typed = 'root.id = "urn:ngsi-ld:Vehicle:" + this.VP.veh.string()\nroot.type = "Vehicle"';
    const form = withSample({ class: "auto", compute: { kind: "bloblang", bloblang: typed } }, drafted);
    expect(form.compute?.bloblang).toBe(typed);
    expect(form.output).toEqual({ type: "Vehicle", mode: "upsert" });
  });

  it("drafts an empty compute node from the sample, typed by the draft", () => {
    for (const compute of [undefined, { kind: "bloblang" as const, bloblang: "  \n" }]) {
      const form = withSample({ class: "auto", compute, output: { type: "", mode: "update-attrs" } }, drafted);
      expect(form.compute?.bloblang).toBe(drafted.bloblang);
      expect(form.output).toEqual({ type: "Hfp", mode: "update-attrs" });
    }
  });

  it("keeps the type the form holds when the kept mapping names none", () => {
    const form = withSample(
      { class: "auto", compute: { kind: "bloblang", bloblang: "root = this" }, output: { type: "Vehicle", mode: "upsert" } },
      drafted,
    );
    expect(form.output?.type).toBe("Vehicle");
  });

  it("reads a literal type from an assignment or the object built, never an attribute's kind", () => {
    expect(literalTypeOf('root.type = "ParkingZone"')).toBe("ParkingZone");
    expect(
      literalTypeOf('root = {"id": this.id, "pm10": {"type": "Property", "value": 1}, "type": "AirQualityObserved"}'),
    ).toBe("AirQualityObserved");
    expect(literalTypeOf('root.type = this.kind')).toBeUndefined();
    expect(literalTypeOf('root.loc = {"type": "GeoProperty", "value": this.geo}')).toBeUndefined();
    expect(literalTypeOf("")).toBeUndefined();
  });
});

describe("the split a test runs follows the source (PL-114, T-5350)", () => {
  const source = (http: Record<string, unknown>, type = "http") => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "s", namespace: "p" },
    spec: { type, http },
  });
  const reads = { class: "auto" as const, source: { dataSourceRef: "s" } };

  it("reads the body format of the http DataSource the draft names", () => {
    expect(sourceBodyOf(reads, [source({ url: "https://x.example/a.csv", format: "csv" })])).toBe("csv");
    expect(sourceBodyOf(reads, [source({ url: "https://x.example/a.csv" })])).toBe("whole");
    expect(sourceBodyOf(reads, [source({}, "mqtt")])).toBeUndefined();
    expect(sourceBodyOf({ class: "auto" }, [source({ format: "csv" })])).toBeUndefined();
    expect(sourceBodyOf(reads, [])).toBeUndefined();
  });

  it("keeps a CSV whole for a whole-body source and splits any sample of a csv source", () => {
    expect(testFormatOf("csv", "whole")).toBe("text");
    expect(testFormatOf("json", "whole")).toBe("json");
    expect(testFormatOf("text", "csv")).toBe("csv");
    expect(testFormatOf("csv", undefined)).toBe("csv");
  });
});
