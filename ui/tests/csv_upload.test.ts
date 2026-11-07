// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/csvUpload.ts.
/**
 * T-3840: a CSV is ranked against the Smart Data Models types of the city subjects, and its rows
 * become entities with a point location from two columns or WKT, written by upsert; the refused
 * rows download as CSV no spreadsheet runs as a formula.
 */
import { describe, expect, it, vi } from "vitest";
import { MIN_SCORE, modelsOf, rankModels, sampleOf, typeNameOf, uploadInput } from "../src/pages/spaces/csvUpload";
import type { CatalogueModel } from "../src/pages/spaces/csvUpload";
import {
  createInBatches,
  LATITUDE,
  LONGITUDE,
  MAX_BYTES,
  parseCsv,
  readTable,
  rejectedCsv,
  rowIds,
  suggestMapping,
  toEntities,
} from "../src/pages/spaces/importRows";
import type { ImportSlot, Send } from "../src/pages/spaces/importRows";

// Attribute names as the catalogue lists them (abridged to what tells the types apart).
const model = (subject: string, name: string, attributes: string[]): CatalogueModel => ({ id: `dataModel.${subject}/${name}`, name, attributes });
const CATALOGUE: CatalogueModel[] = [
  model("Parking", "OffStreetParking", [
    "address", "name", "description", "location", "availableSpotNumber", "totalSpotNumber", "occupiedSpotNumber",
    "openingHours", "maximumAllowedHeight", "chargeType", "allowedVehicleType", "category", "status",
  ]),
  model("Parking", "OnStreetParking", [
    "address", "name", "location", "availableSpotNumber", "totalSpotNumber", "occupiedSpotNumber", "permitActiveHours",
    "maximumParkingDuration", "allowedVehicleType", "chargeType", "category", "status", "areaServed",
  ]),
  model("Parking", "ParkingSpot", ["address", "name", "location", "status", "category", "refParkingSite", "width", "length"]),
  model("Environment", "AirQualityObserved", [
    "address", "location", "dateObserved", "no2", "o3", "pm10", "pm25", "co", "so2", "airQualityIndex", "temperature", "relativeHumidity",
  ]),
  model("Weather", "WeatherObserved", [
    "address", "location", "dateObserved", "temperature", "relativeHumidity", "atmosphericPressure", "windSpeed", "windDirection", "precipitation",
  ]),
  model("Building", "Building", []),
];

describe("which Smart Data Models type a CSV is", () => {
  it("ranks a parking garage file as OffStreetParking first, with the columns mapped", () => {
    const columns = ["Name", "Address", "totalSpotNumber", "available_spot_number", "Opening hours", "lat", "lon"];
    const [best, second] = rankModels(columns, CATALOGUE);
    expect(best.model.name).toBe("OffStreetParking");
    expect(best.score).toBe(1);
    expect(best.mapping).toMatchObject({ Name: "name", Address: "address", available_spot_number: "availableSpotNumber", "Opening hours": "openingHours", lat: "location", lon: "location" });
    expect(second.model.name).toBe("OnStreetParking");
    expect(second.score).toBeLessThan(best.score);
  });

  it("ranks an air quality file as AirQualityObserved, through the date synonym and PM2.5", () => {
    const [best, ...rest] = rankModels(["station", "date", "NO2", "PM10", "PM2.5", "O3"], CATALOGUE);
    expect(best.model.name).toBe("AirQualityObserved");
    expect(best.mapping).toMatchObject({ date: "dateObserved", "PM2.5": "pm25", NO2: "no2" });
    expect(rest).toEqual([]);
  });

  it("offers no type for a file no type names enough of, so it gets its own model", () => {
    expect(rankModels(["foo", "bar", "qux", "name"], CATALOGUE)).toEqual([]);
    // one specific column is not enough, however high its share
    expect(rankModels(["name", "temperature"], CATALOGUE)).toEqual([]);
    // only generic columns: nothing tells a type
    expect(rankModels(["name", "address", "lat", "lon"], CATALOGUE)).toEqual([]);
    expect(rankModels([], CATALOGUE)).toEqual([]);
    expect(rankModels(["no2", "pm10"], [])).toEqual([]);
    for (const match of rankModels(["no2", "pm10", "a", "b", "c"], CATALOGUE)) expect(match.score).toBeGreaterThanOrEqual(MIN_SCORE);
    expect(rankModels(["no2", "pm10", "a", "b", "c", "d"], CATALOGUE)).toEqual([]);
  });

  it("flattens the catalogue index, and an empty or absent one is no model", () => {
    expect(modelsOf({ subjects: [{ models: CATALOGUE.slice(0, 2) }, {}] })).toHaveLength(2);
    expect(modelsOf(undefined)).toEqual([]);
  });
});

const slots: ImportSlot[] = [
  { name: "name", kind: "Property", required: true },
  { name: "totalSpotNumber", range: "integer", kind: "Property", required: false },
  { name: "location", kind: "GeoProperty", required: false },
];
const target = { type: "OffStreetParking", orgDomain: "hel.fi", space: "parking" };

describe("rows of an uploaded CSV", () => {
  it("reads quoted newlines, a header with no rows and an empty file", () => {
    expect(parseCsv('name;note\r\n"A";"two\nlines"\r\n').rows).toEqual([{ name: "A", note: "two\nlines" }]);
    expect(parseCsv("name,lat,lon\n")).toEqual({ columns: ["name", "lat", "lon"], rows: [] });
    expect(parseCsv("")).toEqual({ columns: [], rows: [] });
  });

  it("refuses a file over the size limit before reading it", async () => {
    const big = new File(["x"], "big.csv");
    Object.defineProperty(big, "size", { value: MAX_BYTES + 1 });
    const read = vi.spyOn(big, "arrayBuffer");
    await expect(readTable(big)).rejects.toThrow("50 MB");
    expect(read).not.toHaveBeenCalled();
  });

  it("makes the point location from a latitude and a longitude column, or from WKT", () => {
    const table = parseCsv("name,lat,lon\nA,60.17,24.94\nB,,\nC,95,24\n");
    const mapping = suggestMapping(table.columns, slots);
    expect(mapping).toEqual({ name: "name", lat: LATITUDE, lon: LONGITUDE });
    let minted = 0;
    const { entities, rejected } = toEntities(table, mapping, slots, target, () => `x${(minted += 1)}`);
    expect(entities[0].entity.location).toEqual({ type: "GeoProperty", value: { type: "Point", coordinates: [24.94, 60.17] } });
    expect(entities[1].entity.location).toBeUndefined();
    expect(rejected).toEqual([{ row: 4, id: "urn:ngsi-ld:OffStreetParking:hel.fi:parking:x3", reason: expect.stringContaining("not a latitude and a longitude") }]);

    const wkt = parseCsv('name,geometry\nA,"POINT (24.94 60.17)"\n');
    const out = toEntities(wkt, { name: "name", geometry: "location" }, slots, target, () => "y");
    expect(out.entities[0].entity.location).toEqual({ type: "GeoProperty", value: { type: "Point", coordinates: [24.94, 60.17] } });
  });

  it("offers no latitude target when the type has no geometry", () => {
    expect(suggestMapping(["lat", "lon"], slots.slice(0, 2))).toEqual({ lat: "", lon: "" });
  });

  it("upserts, so the same file uploaded twice writes the same entities", async () => {
    const send = vi.fn<Send>(async () => ({ status: 204 }));
    const answer = await createInBatches(send, "parking", [{ row: 2, entity: { id: "urn:x", type: "T" } }], () => {}, "upsert");
    expect(answer).toEqual({ created: 1, rejected: [] });
    expect(send.mock.calls[0][0].path).toBe("/cs/parking/ngsi-ld/v1/entityOperations/upsert");
  });

  it("writes refused rows no spreadsheet runs as a formula", () => {
    const csv = rejectedCsv([
      { row: 2, id: "=HYPERLINK(\"http://x\")", reason: "+1" },
      { row: 3, id: "-2", reason: "@SUM(A1)" },
      { row: 4, id: "ok", reason: "plain" },
    ]);
    expect(csv.split("\r\n")).toEqual(["row,id,reason", "2,\"'=HYPERLINK(\"\"http://x\"\")\",'+1", "3,'-2,'@SUM(A1)", "4,ok,plain"]);
  });
});

describe("what the upload proposes", () => {
  it("cuts the sample after a whole line, and keeps a short file whole", () => {
    expect(sampleOf("a,b\n1,2\n", 100)).toBe("a,b\n1,2\n");
    expect(sampleOf("a,b\n1,2\n3,4\n", 9)).toBe("a,b\n1,2\n");
    // a cut inside a multi-byte letter still ends on a line
    expect(sampleOf("name\nŽilina\nŽilina\n", 14)).toBe("name\nŽilina\n");
    expect(sampleOf("one long line", 4)).toBe("");
  });

  it("sends the Smart Data Models LinkML beside the sample, and asks for the upload grant", () => {
    expect(uploadInput({ space: "parking", fileName: "p.csv", sample: "a\n", typeName: "OffStreetParking", linkml: "id: x" })).toEqual({
      space: "parking",
      files: [
        { name: "p.csv", content: "a\n" },
        { name: "OffStreetParking.linkml.yaml", content: "id: x" },
      ],
      typeName: "OffStreetParking",
      upload: true,
      propose: true,
    });
    expect(uploadInput({ space: "s", fileName: "p.csv", sample: "a\n", typeName: "Garage" }).files).toHaveLength(1);
  });
});

describe("a file's columns against an inferred model (T-6445)", () => {
  const SLOTS: ImportSlot[] = [
    { name: "N_zov", kind: "Property", required: true, titles: ["Názov"] },
    { name: "Zemepisn_rka", kind: "Property", required: true, titles: ["Zemepisná šírka"] },
    { name: "Zemepisn_d_ka", kind: "Property", required: true, titles: ["Zemepisná dĺžka"] },
    { name: "location", kind: "GeoProperty", required: false },
  ];
  const TABLE = parseCsv("Názov,Zemepisná šírka,Zemepisná dĺžka\nDom kultúry,48.54,19.41\nDom kultúry,48.54,19.41\nŠtadión,48.55,19.42\n");
  const target = { type: "Miesto", orgDomain: "hel.fi", space: "miesta" };

  it("names a row by its content when an earlier row is refused", () => {
    const table = parseCsv("Názov,Zemepisná šírka,Zemepisná dĺžka\n,48.1,19.1\nDom,48.54,19.41\n");
    const alone = { ...table, rows: [table.rows[1]] };
    const ids = (from: typeof table) => toEntities(from, suggestMapping(from.columns, SLOTS), SLOTS, target, rowIds(from));
    const both = ids(table);
    expect(both.rejected).toHaveLength(1);
    expect(both.entities[0].entity.id).toBe(ids(alone).entities[0].entity.id);
  });

  it("meets a column by the slot's title and fills both the coordinate slots and the location", () => {
    const mapping = suggestMapping(TABLE.columns, SLOTS);
    expect(mapping).toEqual({ Názov: "N_zov", "Zemepisná šírka": LATITUDE, "Zemepisná dĺžka": LONGITUDE });
    const { entities, rejected } = toEntities(TABLE, mapping, SLOTS, target, rowIds(TABLE));
    expect(rejected).toEqual([]);
    expect(entities[0]?.entity).toMatchObject({
      N_zov: { value: "Dom kultúry" },
      Zemepisn_rka: { value: 48.54 },
      Zemepisn_d_ka: { value: 19.41 },
      location: { value: { type: "Point", coordinates: [19.41, 48.54] } },
    });
  });

  it("names a row by its content: the same file gives the same ids, a row held twice stays two", () => {
    const ids = (table: typeof TABLE) => toEntities(table, suggestMapping(table.columns, SLOTS), SLOTS, target, rowIds(table)).entities.map((e) => e.entity.id);
    const first = ids(TABLE);
    expect(new Set(first).size).toBe(3);
    expect(ids(parseCsv("Názov,Zemepisná šírka,Zemepisná dĺžka\nDom kultúry,48.54,19.41\nDom kultúry,48.54,19.41\nŠtadión,48.55,19.42\n"))).toEqual(first);
  });

  it("makes a type name of a file or sheet, its diacritics kept as letters", () => {
    expect(typeNameOf("Kultúrne miesta.csv")).toBe("KulturneMiesta");
    expect(typeNameOf("stations.csv")).toBe("Station");
    expect(typeNameOf("Autobus.csv")).toBe("Autobus");
    expect(typeNameOf("analysis.csv")).toBe("Analysis");
    expect(typeNameOf("Vozidlá Mercedes.csv")).toBe("VozidlaMercedes");
    expect(typeNameOf("bike-counters_2026.xlsx")).toBe("BikeCounters2026");
    expect(typeNameOf("2026.csv")).toBeUndefined();
    expect(typeNameOf("名前.csv")).toBeUndefined();
    expect(typeNameOf("entity.csv")).toBeUndefined();
  });
});
