/**
 * The model, over the entities the pipelines of T-2307 actually produced.
 *
 * `fixtures/*.json` are those pipelines' output, recorded on 2026-09-20 from the Bento image the
 * runner uses over answers recorded from the publishers the same day. They are not invented
 * shapes: an attribute the platform would refuse cannot appear in them, and a figure in them is a
 * figure the region published.
 */
import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import type { RichRow } from "@joinedcontext/sdk";
import region from "./fixtures/bbsk-kpi.json";
import city from "./fixtures/banskabystrica-kpi.json";
import { byKey, districtBars, isWhole, LIMITS, splitName, stateOf, toIndicator, UNIT_CODE, unitAsContracted } from "./indicators";
import type { Indicator } from "./indicators";

const rows = (entities: unknown[]): RichRow[] =>
  entities.map((entity) => toRichRow(entity as Record<string, unknown>, "sk"));

const indicators = (entities: unknown[]): Indicator[] =>
  rows(entities)
    .map(toIndicator)
    .filter((indicator): indicator is Indicator => indicator !== null);

const REGION = indicators(region);
const region0 = () => ({ ...(region.find((e) => e.name.value === "obyvatelstvo-stav-kraj") as Record<string, unknown>) });
const city0 = () => ({ ...(city[0] as Record<string, unknown> & { name: { value: string } }) });
const CITY = indicators(city);

describe("reading an indicator", () => {
  it("keeps every entity the two pipelines wrote", () => {
    expect(REGION).toHaveLength(region.length);
    expect(CITY).toHaveLength(city.length);
  });

  it("takes the body from the id and never from the endpoint it arrived on", () => {
    expect(new Set(REGION.map((i) => i.body))).toEqual(new Set(["bbsk"]));
    expect(new Set(CITY.map((i) => i.body))).toEqual(new Set(["banskabystrica"]));
  });

  it("credits an indicator by its space, since both bodies share the Organization's domain", () => {
    // The ids dev serves (T-2455): both projects belong to the one Organization, `hel.fi`.
    const region = toIndicator(
      rows([{ ...region0(), id: "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:bbsk-kpi:obyvatelstvo-stav-kraj" }])[0],
    );
    const mesto = toIndicator(
      rows([{ ...city0(), id: `urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:banskabystrica-kpi:${city0().name.value}` }])[0],
    );
    expect(region?.body).toBe("bbsk");
    expect(mesto?.body).toBe("banskabystrica");
  });

  it("splits a district whose name is three words at the territory and not at the last hyphen", () => {
    expect(splitName("emisie-tuhe-km2-okres-ziar-nad-hronom")).toEqual({
      key: "emisie-tuhe-km2",
      territory: "okres-ziar-nad-hronom",
    });
    expect(splitName("obyvatelstvo-stav-kraj")).toEqual({
      key: "obyvatelstvo-stav",
      territory: "kraj",
    });
    expect(splitName("pm10-24h-mesto")).toEqual({ key: "pm10-24h", territory: "mesto" });
  });

  it("refuses a name whose suffix is no declared territory", () => {
    expect(splitName("obyvatelstvo-stav-okolie")).toBeNull();
  });

  it("carries the value, its unit and its window off the entity", () => {
    const kraj = REGION.find((i) => i.name === "obyvatelstvo-stav-kraj");
    expect(kraj).toMatchObject({
      key: "obyvatelstvo-stav",
      territory: "kraj",
      value: 607581,
      unitCode: "C62",
      period: { start: "2025-01-01T00:00:00Z", end: "2025-12-31T23:59:59Z" },
    });
    expect(kraj?.updatedAt).toMatch(/^\d{4}-\d{2}-\d{2}T/);
    expect(kraj?.formula).not.toBe("");
  });

  it("reads a not-measured indicator as no value and no unit, never as a zero", () => {
    const empty = toIndicator(
      rows([
        {
          id: "urn:ngsi-ld:KeyPerformanceIndicator:banskabystrica.sk:banskabystrica-kpi:pm10-24h-mesto",
          type: "KeyPerformanceIndicator",
          name: { type: "Property", value: "pm10-24h-mesto" },
          currentValue: { type: "Property", value: "not measured" },
          calculationPeriod: {
            type: "Property",
            value: { start: "2026-09-19T07:00:00Z", end: "2026-09-20T07:00:00Z" },
          },
          calculationFormula: { type: "Property", value: "avg(pm10) over AirQualityObserved" },
          updatedAt: { type: "Property", value: { "@type": "DateTime", "@value": "2026-09-20T07:01:00Z" } },
        },
      ])[0],
    );
    expect(empty?.value).toBeNull();
    expect(empty?.unitCode).toBeUndefined();
    expect(empty?.state).toBeNull();
    // The window is still there: the question was asked of a real window and got no reading.
    expect(empty?.period?.end).toBe("2026-09-20T07:00:00Z");
  });
});

describe("what the model refuses", () => {
  const entity = (patch: Record<string, unknown>) => ({
    id: "urn:ngsi-ld:KeyPerformanceIndicator:bbsk.sk:bbsk-kpi:obyvatelstvo-stav-kraj",
    type: "KeyPerformanceIndicator",
    name: { type: "Property", value: "obyvatelstvo-stav-kraj" },
    currentValue: { type: "Property", value: 1, unitCode: "C62" },
    ...patch,
  });

  it("refuses an entity whose name disagrees with its id", () => {
    // Otherwise one indicator could be shown under another's heading, with a real number on it.
    expect(toIndicator(rows([entity({ name: { type: "Property", value: "pm10-24h-mesto" } })])[0])).toBeNull();
  });

  it("refuses a KPI outside the two indicator spaces, whoever's prefix its space carries", () => {
    // Development/10 §1: a body's indicators sit in its `-kpi` space and nowhere else.
    for (const space of ["kpi", "zilina-kpi", "bbsk-kraj", "banskabystrica-mesto", "ovzdusie"]) {
      expect(
        toIndicator(rows([entity({ id: `urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:${space}:obyvatelstvo-stav-kraj` })])[0]),
      ).toBeNull();
    }
  });

  it("refuses an id that is not the six-segment URN", () => {
    expect(
      toIndicator(rows([entity({ id: "urn:ngsi-ld:KeyPerformanceIndicator:bbsk.sk:obyvatelstvo-stav-kraj" })])[0]),
    ).toBeNull();
  });

  it("refuses an entity of another type", () => {
    expect(toIndicator(rows([entity({ type: "AirQualityObserved" })])[0])).toBeNull();
  });
});

describe("the threshold, which is the reader's and not the entity's", () => {
  it("reads PM10 against the directive's limit value and its upper assessment threshold", () => {
    expect(stateOf("pm10-24h", 34.9)).toBe("green");
    expect(stateOf("pm10-24h", 35)).toBe("amber");
    expect(stateOf("pm10-24h", 49.9)).toBe("amber");
    expect(stateOf("pm10-24h", 50)).toBe("red");
  });

  it("reads PM2.5 against its own pair", () => {
    expect(stateOf("pm25-rok", 16.9)).toBe("green");
    expect(stateOf("pm25-rok", 17)).toBe("amber");
    expect(stateOf("pm25-rok", 25)).toBe("red");
  });

  it("gives no state to an indicator no published limit covers", () => {
    for (const key of ["obyvatelstvo-stav", "emisie-tuhe-km2", "spotreba-vody-obyvatel"]) {
      expect(LIMITS[key]).toBeUndefined();
      expect(stateOf(key, 1_000_000)).toBeNull();
    }
    expect(REGION.every((i) => i.state === null)).toBe(true);
  });

  it("gives the city's two air indicators the state their recorded values earn", () => {
    const pm10 = CITY.find((i) => i.name === "pm10-24h-mesto");
    const pm25 = CITY.find((i) => i.name === "pm25-rok-mesto");
    expect(pm10?.value).toBe(35.5);
    expect(pm10?.state).toBe("amber");
    expect(pm25?.state).toBe("green");
  });
});

describe("units", () => {
  it("accepts the unit the contract fixes for each indicator", () => {
    for (const indicator of [...REGION, ...CITY]) {
      if (indicator.value === null) continue;
      expect(indicator.unitCode, indicator.name).toBe(UNIT_CODE[indicator.key]);
      expect(unitAsContracted(indicator)).toBe(true);
    }
  });

  it("refuses to write a contracted unit on a value that does not carry it", () => {
    // A `t/km²` label on a value recorded in something else would be a wrong number, silently.
    const wrong = { ...REGION[0], unitCode: "KGM" };
    expect(unitAsContracted(wrong)).toBe(false);
  });
});

describe("the order the cards are met in", () => {
  it("puts the thresholded indicators first and the whole territory before its districts", () => {
    const groups = byKey([...REGION, ...CITY]);
    expect(groups.map((g) => g.key).slice(0, 2)).toEqual(["pm10-24h", "pm25-rok"]);
    const emissions = groups.find((g) => g.key === "emisie-tuhe-km2");
    expect(emissions?.rows[0].territory).toBe("kraj");
    expect(emissions?.rows).toHaveLength(14);
  });
});

describe("the bars of one indicator", () => {
  const population = REGION.filter((indicator) => indicator.key === "obyvatelstvo-stav");

  it("draws every measured district, largest first, and leaves the whole region out", () => {
    const bars = districtBars(population);
    expect(bars).toHaveLength(13);
    expect(bars.some((bar) => isWhole(bar.territory))).toBe(false);
    for (let i = 1; i < bars.length; i++) {
      expect(bars[i - 1].value).toBeGreaterThanOrEqual(bars[i].value);
    }
  });

  it("gives a district that was not measured no bar, never a zero one", () => {
    const quiet = population.map((indicator) =>
      indicator.territory === "okres-brezno" ? { ...indicator, value: null } : indicator,
    );
    const bars = districtBars(quiet);
    expect(bars).toHaveLength(12);
    expect(bars.find((bar) => bar.territory === "okres-brezno")).toBeUndefined();
  });

  it("has nothing to draw for the city, which publishes only its whole territory", () => {
    expect(districtBars(CITY)).toEqual([]);
  });
});

describe("an entity's odd shapes", () => {
  const ID = "urn:ngsi-ld:KeyPerformanceIndicator:bbsk.sk:bbsk-kpi:obyvatelstvo-stav-kraj";
  const read = (attrs: Record<string, unknown>) =>
    toIndicator(toRichRow({ id: ID, type: "KeyPerformanceIndicator", ...attrs }, "sk"));
  const NAME = { type: "Property", value: "obyvatelstvo-stav-kraj" };

  it("takes the first of several values, and a relationship's object as text", () => {
    expect(read({ name: [{ ...NAME, datasetId: "urn:a" }, { type: "Property", value: "x", datasetId: "urn:b" }] })?.name).toBe("obyvatelstvo-stav-kraj");
    expect(read({ name: { type: "Relationship", object: "obyvatelstvo-stav-kraj" } })?.name).toBe("obyvatelstvo-stav-kraj");
    expect(read({ name: { type: "Property", value: 7 } })).toBeNull();
  });

  it("refuses a name whose suffix is no territory it knows", () => {
    const id = "urn:ngsi-ld:KeyPerformanceIndicator:bbsk.sk:bbsk-kpi:obyvatelstvo-stav-mars";
    expect(toIndicator(toRichRow({ id, type: "KeyPerformanceIndicator", name: { type: "Property", value: "obyvatelstvo-stav-mars" } }, "sk"))).toBeNull();
  });

  it("leaves out a window that is no window, and reads a missing formula as none", () => {
    expect(read({ name: NAME, calculationPeriod: { type: "Property", value: "2025" } })).toMatchObject({ period: undefined, formula: "" });
    expect(read({ name: NAME, calculationPeriod: { type: "Property", value: { start: 2025, end: "2026" } } })?.period).toBeUndefined();
  });
});
