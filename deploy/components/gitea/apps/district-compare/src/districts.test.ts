import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import type { DistrictOutput, RowsByType } from "./districts";
import {
  choroplethValue,
  defaultSelection,
  districtName,
  hasPerKm2,
  measurePerKm2,
  measureValue,
  readHash,
  readView,
  sortDistrictsByMeasure,
  toInput,
  writeHash,
  writeView,
} from "./districts";

describe("districtName", () => {
  it("uses formatted string name when available", () => {
    const row: Row = {
      id: "urn:ngsi-ld:AdministrativeArea:102",
      type: "AdministrativeArea",
      districtCode: "102",
      name: "Kamppi",
    };
    expect(districtName(row)).toBe("Kamppi");
  });

  it("falls back to districtCode when name is missing or raw JSON/URN", () => {
    const rowWithUrnName: Row = {
      id: "urn:ngsi-ld:AdministrativeArea:103",
      type: "AdministrativeArea",
      districtCode: "103",
      name: "urn:ngsi-ld:AdministrativeArea:103",
    };
    expect(districtName(rowWithUrnName)).toBe("103");

    const rowWithEmptyName: Row = {
      id: "urn:ngsi-ld:AdministrativeArea:104",
      type: "AdministrativeArea",
      districtCode: "104",
      name: null,
    };
    expect(districtName(rowWithEmptyName)).toBe("104");
  });

  it("falls back to id when neither name nor districtCode is provided", () => {
    const row: Row = {
      id: "urn:ngsi-ld:AdministrativeArea:fallback-id",
      type: "AdministrativeArea",
      name: null,
    };
    expect(districtName(row)).toBe("urn:ngsi-ld:AdministrativeArea:fallback-id");
  });
});

describe("toInput", () => {
  it("drops non-district divisionLevel rows and keeps district rows", () => {
    const rows: RowsByType = {
      districts: [
        {
          id: "urn:ngsi-ld:AdministrativeArea:101",
          type: "AdministrativeArea",
          districtCode: "101",
          divisionLevel: "district",
          name: "Kallio",
          location: { type: "Point", coordinates: [24.95, 60.18] },
        },
        {
          id: "urn:ngsi-ld:AdministrativeArea:201",
          type: "AdministrativeArea",
          districtCode: "201",
          divisionLevel: "subdistrict",
          name: "Alppiharju",
          location: { type: "Point", coordinates: [24.95, 60.19] },
        },
      ],
      events: [],
      bikes: [],
      alerts: [],
      air: [],
    };
    const input = toInput(rows);
    expect(input.districts).toHaveLength(1);
    expect(input.districts[0].code).toBe("101");
    expect(input.districts[0].name).toBe("Kallio");
  });

  it("handles missing location and represents unlocated entities out of range", () => {
    const rows: RowsByType = {
      districts: [
        {
          id: "urn:ngsi-ld:AdministrativeArea:101",
          type: "AdministrativeArea",
          districtCode: "101",
          divisionLevel: "district",
          name: "Kallio",
          location: null,
        },
      ],
      events: [
        { id: "e1", type: "Event", location: { type: "Point", coordinates: [24.94, 60.17] } },
        { id: "e2", type: "Event", location: null },
      ],
      bikes: [
        {
          id: "b1",
          type: "BikeHireDockingStation",
          totalSlotNumber: 20,
          location: { type: "Point", coordinates: [24.94, 60.17] },
        },
        {
          id: "b2",
          type: "BikeHireDockingStation",
          totalSlotNumber: "15",
          location: null,
        },
      ],
      alerts: [
        { id: "a1", type: "Alert", location: { type: "Point", coordinates: [24.94, 60.17] } },
        { id: "a2", type: "Alert", location: null },
      ],
      air: [
        {
          id: "air1",
          type: "AirQualityObserved",
          pm25: 12.5,
          airQualityIndex: 2,
          location: { type: "Point", coordinates: [24.94, 60.17] },
        },
        {
          id: "air2",
          type: "AirQualityObserved",
          pm25: "18.0",
          airQualityIndex: "3",
          location: null,
        },
      ],
    };
    const input = toInput(rows);
    expect(input.districts[0].geometry).toBeNull();
    expect(input.events[0]).toEqual([24.94, 60.17]);
    expect(input.events[1]).toEqual([999.0, 999.0]);
    expect(input.bikes[0]).toEqual({ at: [24.94, 60.17], slots: 20 });
    expect(input.bikes[1]).toEqual({ at: [999.0, 999.0], slots: 15 });
    expect(input.alerts[0]).toEqual([24.94, 60.17]);
    expect(input.alerts[1]).toEqual([999.0, 999.0]);
    expect(input.air[0]).toEqual({ at: [24.94, 60.17], pm25: 12.5, aqi: 2 });
    expect(input.air[1]).toEqual({ at: [999.0, 999.0], pm25: 18.0, aqi: 3 });
  });
});

describe("measure helpers", () => {
  const sampleDistrict: DistrictOutput = {
    code: "101",
    name: "Kallio",
    areaKm2: 2.0,
    events: 10,
    bikes: 4,
    bikeSlots: 80,
    alerts: 2,
    pm25: 14.2,
    aqi: 2.5,
    perKm2: {
      events: 5.0,
      bikes: 2.0,
      bikeSlots: 40.0,
      alerts: 1.0,
    },
    rank: {
      events: 1,
      bikes: 1,
      bikeSlots: 1,
      alerts: 1,
      pm25: 1,
      aqi: 1,
    },
  };

  it("reads measure values accurately", () => {
    expect(measureValue(sampleDistrict, "events")).toBe(10);
    expect(measureValue(sampleDistrict, "bikes")).toBe(4);
    expect(measureValue(sampleDistrict, "bikeSlots")).toBe(80);
    expect(measureValue(sampleDistrict, "alerts")).toBe(2);
    expect(measureValue(sampleDistrict, "pm25")).toBe(14.2);
    expect(measureValue(sampleDistrict, "aqi")).toBe(2.5);
  });

  it("distinguishes measures that have per km² rates", () => {
    expect(hasPerKm2("events")).toBe(true);
    expect(hasPerKm2("bikes")).toBe(true);
    expect(hasPerKm2("bikeSlots")).toBe(true);
    expect(hasPerKm2("alerts")).toBe(true);
    expect(hasPerKm2("pm25")).toBe(false);
    expect(hasPerKm2("aqi")).toBe(false);

    expect(measurePerKm2(sampleDistrict, "events")).toBe(5.0);
    expect(measurePerKm2(sampleDistrict, "pm25")).toBeNull();
  });

  it("uses density for choropleth counts and mean for air quality", () => {
    expect(choroplethValue(sampleDistrict, "events")).toBe(5.0);
    expect(choroplethValue(sampleDistrict, "pm25")).toBe(14.2);
  });
});

describe("defaultSelection", () => {
  it("returns empty array when output is null or has no districts", () => {
    expect(defaultSelection(null)).toEqual([]);
    expect(defaultSelection({ districts: [], outside: { events: 0, bikes: 0, alerts: 0, air: 0 } })).toEqual([]);
  });

  it("returns district alone when only one district exists", () => {
    const single: DistrictOutput = {
      code: "101",
      name: "Kallio",
      areaKm2: 1.0,
      events: 5,
      bikes: 1,
      bikeSlots: 20,
      alerts: 0,
      pm25: null,
      aqi: null,
      perKm2: { events: 5, bikes: 1, bikeSlots: 20, alerts: 0 },
      rank: { events: 1, bikes: 1, bikeSlots: 1, alerts: 1, pm25: null, aqi: null },
    };
    expect(defaultSelection({ districts: [single], outside: { events: 0, bikes: 0, alerts: 0, air: 0 } })).toEqual([
      "101",
    ]);
  });

  it("selects the two districts with the most events, breaking ties by name", () => {
    const makeDistrict = (code: string, name: string, events: number): DistrictOutput => ({
      code,
      name,
      areaKm2: 1.0,
      events,
      bikes: 0,
      bikeSlots: 0,
      alerts: 0,
      pm25: null,
      aqi: null,
      perKm2: { events, bikes: 0, bikeSlots: 0, alerts: 0 },
      rank: { events: 1, bikes: 1, bikeSlots: 1, alerts: 1, pm25: null, aqi: null },
    });

    const d1 = makeDistrict("101", "Kallio", 10);
    const d2 = makeDistrict("102", "Kamppi", 25);
    const d3 = makeDistrict("103", "Töölö", 15);
    const d4 = makeDistrict("104", "Pasila", 5);

    const selection = defaultSelection({
      districts: [d1, d2, d3, d4],
      outside: { events: 0, bikes: 0, alerts: 0, air: 0 },
    });
    expect(selection).toEqual(["102", "103"]);

    // Tie-breaker by name
    const tieA = makeDistrict("105", "Alppiharju", 20);
    const tieB = makeDistrict("106", "Berghäll", 20);
    const tieSelection = defaultSelection({
      districts: [tieB, tieA, d4],
      outside: { events: 0, bikes: 0, alerts: 0, air: 0 },
    });
    expect(tieSelection).toEqual(["105", "106"]);
  });
});

describe("hash view state", () => {
  it("round trips selected codes and measure", () => {
    const state = { selectedCodes: ["101", "102"], measure: "bikes" as const };
    const hash = writeHash(state);
    expect(hash).toBe("#compare?d=101%2C102&m=bikes");
    expect(readHash(hash)).toEqual(state);
  });

  it("handles empty selection and aliases", () => {
    const state = { selectedCodes: [], measure: "events" as const };
    const hash = writeView(state);
    expect(hash).toBe("#compare?m=events");
    expect(readView(hash)).toEqual(state);
  });

  it("ignores junk and malformed parameters in hash", () => {
    expect(readHash("#compare?m=unknown_measure&d=101")).toEqual({
      selectedCodes: ["101"],
      measure: "events",
    });
    expect(readHash("#compare?d=101, ,102,,&m=alerts")).toEqual({
      selectedCodes: ["101", "102"],
      measure: "alerts",
    });
    expect(readHash("")).toEqual({ selectedCodes: [], measure: "events" });
    expect(readHash("#compare")).toEqual({ selectedCodes: [], measure: "events" });
  });
});

describe("sortDistrictsByMeasure", () => {
  it("sorts districts by competition rank ascending, placing null ranks last", () => {
    const makeDistrict = (code: string, name: string, rankVal: number | null): DistrictOutput => ({
      code,
      name,
      areaKm2: 1.0,
      events: 0,
      bikes: 0,
      bikeSlots: 0,
      alerts: 0,
      pm25: rankVal,
      aqi: null,
      perKm2: { events: 0, bikes: 0, bikeSlots: 0, alerts: 0 },
      rank: { events: 1, bikes: 1, bikeSlots: 1, alerts: 1, pm25: rankVal, aqi: null },
    });

    const d1 = makeDistrict("101", "Kallio", 3);
    const d2 = makeDistrict("102", "Kamppi", 1);
    const d3 = makeDistrict("103", "Töölö", null);
    const d4 = makeDistrict("104", "Alppiharju", 1);

    const sorted = sortDistrictsByMeasure([d1, d2, d3, d4], "pm25");
    expect(sorted.map((d) => d.code)).toEqual(["104", "102", "101", "103"]);
  });
});
