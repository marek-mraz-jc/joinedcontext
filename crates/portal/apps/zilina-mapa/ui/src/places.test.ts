import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { byOrder, featuresOf, matches, placeOf } from "./places";
import type { Place } from "./places";
import { AIR, MONUMENTS, STATIONS } from "./fixtures/verejne";

const monument = (index: number) => placeOf(toRichRow(MONUMENTS[index]), "monument", "sk");
const station = (index: number) => placeOf(toRichRow(STATIONS[index]), "station", "sk");
const air = () => placeOf(toRichRow(AIR[0]), "air", "sk");

describe("placeOf", () => {
  it("reads a monument's register entries and its building, the name capitalised", () => {
    expect(monument(1)).toMatchObject({
      name: "Jezitský kláštor",
      monumentNumber: "1395/1",
      monumentKind: "KLÁŠTOR JEZUITOV",
      address: "Mariánske námestie 158/23, Žilina",
      cadastralArea: "Žilina",
    });
    expect(monument(1).coordinates).not.toBeNull();
  });

  it("keeps a monument without an address without a position", () => {
    expect(monument(0)).toMatchObject({ name: "Trojičný stĺp", address: null, coordinates: null });
  });

  it("reads each pollutant with its own unit and hour, carbon monoxide in mg/m³", () => {
    const station = air();
    expect(station.readings.pm10).toEqual({ value: 32.341, unit: "µg/m³", at: "2026-10-06T18:00:00Z" });
    expect(station.readings.o3).toEqual({ value: 11.8076, unit: "µg/m³", at: "2026-10-05T17:00:00Z" });
    expect(station.readings.co).toEqual({ value: 0.63452, unit: "mg/m³", at: "2026-10-06T18:00:00Z" });
  });

  it("reads a station's trains leaving today", () => {
    expect(STATIONS.map((_, index) => station(index)).find((p) => p.name === "Žilina")?.departures).toBe(166);
  });
});

describe("filters", () => {
  it("finds every word in the name, address, object or style, without diacritics", () => {
    expect(matches(monument(1), "jezuitov")).toBe(true);
    expect(matches(monument(1), "marianske 158")).toBe(true);
    expect(matches(monument(2), "kastiel")).toBe(true);
    expect(matches(monument(0), "marianske")).toBe(false);
    expect(matches(monument(0), "  ")).toBe(true);
  });

  it("orders stations busiest first and the rest by name", () => {
    const stations = STATIONS.map((_, index) => station(index)).sort(byOrder).map((p) => p.name);
    expect(stations[0]).toBe("Žilina");
    expect(stations[1]).toBe("Brodno");
    const monuments = MONUMENTS.map((_, index) => monument(index)).sort(byOrder).map((p) => p.name);
    expect(monuments).toEqual([...monuments].sort((a, b) => (a ?? "").localeCompare(b ?? "", "sk")));
  });

  it("draws only the places with a position, the picked one marked", () => {
    const features = featuresOf([monument(0), monument(1)], monument(1).id).features;
    expect(features).toHaveLength(1);
    expect(features[0].properties).toMatchObject({ picked: true });
  });
});

describe("what a source writes otherwise", () => {
  const P = (value: unknown) => ({ type: "Property", value });
  const at = (coordinates: unknown) => ({ type: "GeoProperty", value: { type: "Point", coordinates } });
  const row = (attrs: Record<string, unknown>) => toRichRow({ id: "urn:ngsi-ld:PointOfInterest:x", type: "PointOfInterest", ...attrs } as never);

  it("reads a name in the reader's language, else Slovak, else any, and a blank one as none", () => {
    const lang = (languageMap: Record<string, string>) => ({ type: "LanguageProperty", languageMap });
    expect(placeOf(row({ name: lang({ en: "Castle", sk: "Hrad" }) }), "station", "en").name).toBe("Castle");
    expect(placeOf(row({ name: lang({ sk: "Hrad" }) }), "station", "en").name).toBe("Hrad");
    expect(placeOf(row({ name: lang({ de: "Burg" }) }), "station", "en").name).toBe("Burg");
    expect(placeOf(row({ name: lang({ sk: "  " }) }), "station", "sk").name).toBeNull();
    expect(placeOf(row({ name: P(" Hrad ") }), "station", "sk").name).toBe("Hrad");
    expect(placeOf(row({}), "monument", "sk").name).toBeNull();
  });

  it("reads a number as text, a blank text and an object as none, and a number only when finite", () => {
    const place = placeOf(row({ address: P(42), monumentNumber: P("  "), style: P({ a: 1 }), dailyDepartures: P("58") }), "station", "sk");
    expect(place).toMatchObject({ address: "42", monumentNumber: null, style: null, departures: null });
  });

  it("places only a point on the earth", () => {
    expect(placeOf(row({ location: at([18.7, 49.2]) }), "station", "sk").coordinates).toEqual([18.7, 49.2]);
    expect(placeOf(row({ location: at([200, 49.2]) }), "station", "sk").coordinates).toBeNull();
    expect(placeOf(row({ location: at([18.7, 95]) }), "station", "sk").coordinates).toBeNull();
    expect(placeOf(row({ location: at(["18.7", 49.2]) }), "station", "sk").coordinates).toBeNull();
    expect(placeOf(row({ location: at("18.7,49.2") }), "station", "sk").coordinates).toBeNull();
    expect(placeOf(row({ location: { type: "GeoProperty", value: { type: "LineString", coordinates: [] } } }), "station", "sk").coordinates).toBeNull();
  });

  it("reads a reading in µg/m³ unless it says otherwise, at its hour or the station's, and drops one that is no number", () => {
    const station = placeOf(
      row({ dateObserved: P("2026-10-06T18:00:00Z"), pm10: P(20), no2: { type: "Property", value: 3, unitCode: "XYZ", observedAt: "2026-10-06T17:00:00Z" }, o3: P("high"), co: P(Number.NaN) }),
      "air",
      "sk",
    );
    expect(station.readings).toEqual({
      pm10: { value: 20, unit: "µg/m³", at: "2026-10-06T18:00:00Z" },
      no2: { value: 3, unit: "XYZ", at: "2026-10-06T17:00:00Z" },
    });
    expect(placeOf(row({ pm10: P(20) }), "air", "sk").readings.pm10?.at).toBeNull();
  });

  it("orders a station without a count after one with, and an unnamed place last", () => {
    const place = (kind: Place["kind"], name: string | null, departures: number | null) => ({ ...monument(0), kind, name, departures });
    expect([place("station", "A", null), place("station", "B", 3)].sort(byOrder).map((p) => p.name)).toEqual(["B", "A"]);
    expect([place("station", "B", null), place("station", "A", 3)].sort(byOrder).map((p) => p.name)).toEqual(["A", "B"]);
    expect([place("monument", null, null), place("monument", "A", null)].sort(byOrder).map((p) => p.name)).toEqual(["A", null]);
    expect([place("monument", "A", null), place("monument", null, null)].sort(byOrder).map((p) => p.name)).toEqual(["A", null]);
    expect(byOrder(place("monument", null, null), place("monument", null, null))).toBe(0);
  });
});
