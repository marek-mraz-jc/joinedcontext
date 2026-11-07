/** The rows of praha-mesto as the screen reads them (T-2917), over what the pipelines write. */
import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import bikes from "./fixtures/bikes.json";
import parking from "./fixtures/parking.json";
import air from "./fixtures/air.json";
import { between, bikeHistogram, histogramBars, bikePoints, bikeTotals, legendOf, matching, nameOf, stepLabels, toAirStation, toBikeStation, toCarPark } from "./praha";
import type { BikeStation } from "./praha";

const rows = (entities: unknown[]) => entities.map((entity) => toRichRow(entity as Record<string, unknown>, "cs"));

describe("bike stations", () => {
  it("reads every named station and drops one the stations feed never named", () => {
    const stations = rows(bikes).map((row) => toBikeStation(row, "cs"));
    expect(stations.filter(Boolean)).toHaveLength(3);
    expect(stations[3]).toBeNull();
  });

  it("counts only what stations in service hold", () => {
    const stations = rows(bikes).map((row) => toBikeStation(row, "cs")).filter((s) => s !== null);
    expect(bikeTotals(stations)).toEqual({ stations: 3, working: 3, bikes: 1, docks: 29 });
    stations[1].working = false;
    expect(bikeTotals(stations)).toEqual({ stations: 3, working: 2, bikes: 0, docks: 20 });
  });

  it("leaves out a count that is not a whole number instead of showing a zero", () => {
    const [first] = rows([{ ...bikes[0], availableBikeNumber: { type: "Property", value: -2 } }]);
    expect(toBikeStation(first, "cs")?.bikes).toBeNull();
  });
});

describe("park and ride", () => {
  it("shows the live counts where the counters feed them and nothing invented where they do not", () => {
    const [counted, plain] = rows(parking).map((row) => toCarPark(row, "cs"));
    expect(counted).toMatchObject({ capacity: 92, free: 23, occupied: 55 });
    expect(plain).toMatchObject({ capacity: 131, free: null, occupied: null });
  });
});

describe("air quality", () => {
  it("keeps the pollutants a station measured and the hour of each", () => {
    const [libus, legerova] = rows(air).map((row) => toAirStation(row, "cs"));
    expect(Object.keys(libus?.readings ?? {}).sort()).toEqual(["no2", "o3", "pm10", "pm25"]);
    expect(libus?.readings.pm10).toEqual({ value: 4.9, at: "2026-09-25T03:00:00Z" });
    expect(Object.keys(legerova?.readings ?? {})).toEqual(["no2"]);
  });

  it("drops a station with no reading at all", () => {
    const [bare] = rows([{ id: air[0].id, type: "AirQualityObserved", name: air[0].name }]);
    expect(toAirStation(bare, "cs")).toBeNull();
  });
});

describe("names and search", () => {
  it("reads the name in the asked language, then Czech", () => {
    const [row] = rows([{ id: "urn:ngsi-ld:X:a:b:c", type: "X", name: { type: "LanguageProperty", languageMap: { cs: "Náměstí", en: "Square" } } }]);
    expect(nameOf(row, "en")).toBe("Square");
    expect(nameOf(row, "de")).toBe("Náměstí");
  });

  it("finds a station without its diacritics and sorts by name", () => {
    const stations = rows(bikes).map((row) => toBikeStation(row, "cs")).filter((s) => s !== null);
    expect(matching(stations, "vrsovicke", "cs-CZ").map((s) => s.name)).toEqual(["P10-Vršovické náměstí - REST. WAIKIKI"]);
    expect(matching(stations, "", "cs-CZ").map((s) => s.name)[0]).toBe("P10-Čechovo náměstí");
  });
});

describe("the map and the histogram", () => {
  const station = (bikes: number | null, working = true): BikeStation => ({
    id: `s-${bikes}-${working}`,
    name: "s",
    bikes,
    docks: 1,
    working,
    at: [14.4, 50.1],
  });

  it("counts stations in service into the steps and leaves out one that lends nothing or reports nothing", () => {
    const stations = [station(0), station(2), station(3), station(10), station(11), station(40), station(5, false), station(null)];
    expect(bikeHistogram(stations)).toEqual([1, 1, 1, 1, 2]);
    expect(stepLabels([0, 1, 3, 6, 11])).toEqual(["0", "1–2", "3–5", "6–10", "11+"]);
  });

  it("puts on the map only the stations in service that have a point and a count", () => {
    const points = bikePoints([station(4), station(4, false), station(null), { ...station(2), at: undefined }]);
    expect(points).toEqual([{ id: "s-4-true", at: [14.4, 50.1], value: 4 }]);
  });

  it("reads a point location and refuses one outside the WGS84 range", () => {
    const [inside, outside] = rows([
      { ...bikes[0], location: { type: "GeoProperty", value: { type: "Point", coordinates: [14.42, 50.08] } } },
      { ...bikes[0], location: { type: "GeoProperty", value: { type: "Point", coordinates: [514.42, 50.08] } } },
    ]).map((row) => toBikeStation(row, "cs"));
    expect(inside?.at).toEqual([14.42, 50.08]);
    expect(outside?.at).toBeUndefined();
  });

  it("colours each histogram bar with its step's legend colour, the one the map dots carry", () => {
    const legend = legendOf([0, 1, 3], "#000000", "#ff8800");
    expect(histogramBars([5, 0, 2], legend)).toEqual([
      { value: 5, itemStyle: { color: "#000000" } },
      { value: 0, itemStyle: { color: "#804400" } },
      { value: 2, itemStyle: { color: "#ff8800" } },
    ]);
  });

  it("runs the legend from the low colour to the high one", () => {
    const legend = legendOf([0, 1, 3], "#000000", "#ff8800");
    expect(legend.map((step) => step.colour)).toEqual(["#000000", "#804400", "#ff8800"]);
    expect(legend.map((step) => step.label)).toEqual(["0", "1–2", "3+"]);
    // A token that is not a hex colour draws the high end rather than a broken one.
    expect(between("rebeccapurple", "#ff8800", 0.5)).toBe("#ff8800");
  });
});

describe("the edges of a row", () => {
  const named = (name: unknown) => rows([{ id: "urn:ngsi-ld:X:a:b:c", type: "X", name }])[0];

  it("names a row in its first language, as plain text, and not at all when blank", () => {
    expect(nameOf(named({ type: "LanguageProperty", languageMap: { de: "Platz" } }), "en")).toBe("Platz");
    expect(nameOf(named({ type: "Property", value: "Plain" }), "en")).toBe("Plain");
    expect(nameOf(named({ type: "Property", value: "  " }), "en")).toBeNull();
    expect(nameOf(rows([{ id: "urn:ngsi-ld:X:a:b:d", type: "X" }])[0], "en")).toBeNull();
  });

  it("reads no row of another type, and none without a name", () => {
    const [bike] = rows([bikes[0]]);
    expect(toCarPark(bike, "cs")).toBeNull();
    expect(toAirStation(bike, "cs")).toBeNull();
    expect(toBikeStation(rows([parking[0]])[0], "cs")).toBeNull();
    const nameless = (entity: Record<string, unknown>) => rows([{ ...entity, name: { type: "Property", value: "" } }])[0];
    expect(toCarPark(nameless(parking[0]), "cs")).toBeNull();
    expect(toAirStation(nameless(air[0]), "cs")).toBeNull();
  });

  it("keeps a broken date and a shape that is no point out", () => {
    const [row] = rows([{ ...bikes[0], dateModified: { type: "Property", value: "yesterday" }, location: { type: "GeoProperty", value: { type: "LineString", coordinates: [[14, 50]] } } }]);
    expect(toBikeStation(row, "cs")).toMatchObject({ reportedAt: undefined, at: undefined });
    const [half] = rows([{ ...bikes[0], location: { type: "GeoProperty", value: { type: "Point", coordinates: ["14", 50] } } }]);
    expect(toBikeStation(half, "cs")?.at).toBeUndefined();
  });

  it("dates a reading by its station when the reading carries no hour of its own", () => {
    const [row] = rows([{ id: air[0].id, type: "AirQualityObserved", name: air[0].name, pm10: { type: "Property", value: 7 }, dateObserved: { type: "Property", value: "2026-09-25T05:00:00Z" } }]);
    expect(toAirStation(row, "cs")?.readings.pm10).toEqual({ value: 7, at: "2026-09-25T05:00:00Z" });
    const [negative] = rows([{ id: air[0].id, type: "AirQualityObserved", name: air[0].name, pm10: { type: "Property", value: -1 } }]);
    expect(toAirStation(negative, "cs")).toBeNull();
  });

  it("counts a station in service that reports no bikes or docks as none", () => {
    const station: BikeStation = { id: "x", name: "x", bikes: null, docks: null, working: true };
    expect(bikeTotals([station])).toEqual({ stations: 1, working: 1, bikes: 0, docks: 0 });
  });

  it("leaves a count below the first step out, and colours bars past the legend with its last colour", () => {
    const station: BikeStation = { id: "x", name: "x", bikes: 0, docks: 1, working: true };
    expect(bikeHistogram([station], [1, 3])).toEqual([0, 0]);
    expect(histogramBars([1, 2], [{ colour: "#111111" }])).toEqual([
      { value: 1, itemStyle: { color: "#111111" } },
      { value: 2, itemStyle: { color: "#111111" } },
    ]);
    expect(histogramBars([1], [])).toEqual([{ value: 1, itemStyle: { color: "" } }]);
    expect(legendOf([0], "#000000", "#ffffff")).toEqual([{ from: 0, label: "0+", colour: "#ffffff" }]);
    expect(between("#000000", "#ffffff", 2)).toBe("#ffffff");
    expect(between("#000000", "#ffffff", -1)).toBe("#000000");
  });
});
