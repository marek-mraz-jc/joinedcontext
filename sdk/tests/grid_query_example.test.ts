import { describe, expect, it } from "vitest";
import { toRichRow } from "../src/grid/model";
import { queryExample } from "../src/grid/queryExample";

// T-5415: the "Edit as text" hint names an attribute the shown type has, never a fixed city's.
describe("queryExample", () => {
  it("builds a traffic counter's example from its own count", () => {
    const row = toRichRow({
      id: "urn:ngsi-ld:TrafficFlowObserved:za:1",
      type: "TrafficFlowObserved",
      name: { type: "Property", value: "Sčítač dopravy Hálkova" },
      intensity: { type: "Property", value: 121 },
    });
    expect(queryExample(["name", "intensity"], row)).toBe("intensity>=121");
  });

  it("quotes a short text when an air station has no number on the page", () => {
    const row = toRichRow({
      id: "urn:ngsi-ld:AirQualityObserved:praha:1",
      type: "AirQualityObserved",
      dateObserved: { type: "Property", value: { "@type": "DateTime", "@value": "2026-10-23T03:00:00Z" } },
      stationCode: { type: "Property", value: "ALEGA" },
    });
    const example = queryExample(["dateObserved", "stationCode"], row);
    expect(example).toBe('stationCode=="ALEGA"');
    expect(example).not.toMatch(/pm10|Espoo/);
  });

  it("gives none without a row or a plain value", () => {
    expect(queryExample(["pm10"], undefined)).toBeUndefined();
    const row = toRichRow({ id: "urn:x:1", type: "T", note: { type: "Property", value: 'say "hi"' } });
    expect(queryExample(["note", "bad[name]"], row)).toBeUndefined();
  });
});
