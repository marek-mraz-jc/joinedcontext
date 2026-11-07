import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { byOrder, featuresOf, matches, placeOf, upcoming } from "./places";
import { EVENTS, SCHOOLS } from "./fixtures/verejne";

const event = (index: number) => placeOf(toRichRow(EVENTS[index]), "event", "sk");
const school = (index: number) => placeOf(toRichRow(SCHOOLS[index]), "school", "sk");

describe("placeOf", () => {
  it("reads an event's name, days, address and position", () => {
    expect(event(0)).toMatchObject({
      name: "Radvanský jarmok",
      startDate: "2026-10-08",
      endDate: "2026-10-10",
      address: "Námestie SNP, Banská Bystrica",
      coordinates: [19.1459, 48.7357],
    });
  });

  it("keeps a missing value missing, and reads a name in the reader's language, else Slovak, else any", () => {
    expect(event(2).coordinates).toBeNull();
    const row = (fields: Record<string, unknown>) => toRichRow({ id: "urn:ngsi-ld:Event:x", type: "Event", ...fields });
    expect(placeOf(row({ name: { type: "LanguageProperty", languageMap: { en: "Fair", sk: "Jarmok" } } }), "event", "en").name).toBe("Fair");
    expect(placeOf(row({ name: { type: "LanguageProperty", languageMap: { sk: "Jarmok" } } }), "event", "en").name).toBe("Jarmok");
    expect(placeOf(row({ name: { type: "LanguageProperty", languageMap: { de: "Markt" } } }), "event", "en").name).toBe("Markt");
    expect(placeOf(row({ name: { type: "LanguageProperty", languageMap: { sk: "  " } } }), "event", "sk").name).toBeNull();
    expect(placeOf(row({ name: { type: "Property", value: "  " }, address: { type: "Property", value: 12 } }), "event", "sk")).toMatchObject({ name: null, address: "12" });
    expect(placeOf(row({ address: { type: "Property", value: true } }), "event", "sk").address).toBeNull();
    // T-4329: the city now writes the Smart Data Models PostalAddress; it reads as one line.
    const postal = { streetAddress: "Námestie SNP 1", postalCode: "97401", addressLocality: "Banská Bystrica", addressCountry: "SK" };
    expect(placeOf(row({ address: { type: "Property", value: postal } }), "event", "sk").address).toBe("Námestie SNP 1, 97401 Banská Bystrica");
    expect(placeOf(row({ address: { type: "Property", value: {} } }), "event", "sk").address).toBeNull();
  });

  it("puts on the map only a point inside the world", () => {
    const at = (value: unknown) => placeOf(toRichRow({ id: "urn:ngsi-ld:Event:x", type: "Event", location: { type: "GeoProperty", value } }), "event", "sk").coordinates;
    expect(at({ type: "Point", coordinates: [19.1, 48.7] })).toEqual([19.1, 48.7]);
    expect(at({ type: "Point", coordinates: [190, 48.7] })).toBeNull();
    expect(at({ type: "Point", coordinates: ["19", 48.7] })).toBeNull();
    expect(at({ type: "Polygon", coordinates: [] })).toBeNull();
  });
});

describe("filters", () => {
  it("keeps events from today on, and every other kind", () => {
    expect(upcoming(event(0), "2026-10-09")).toBe(true);
    expect(upcoming(event(0), "2026-10-11")).toBe(false);
    expect(upcoming(event(1), "2026-10-06")).toBe(false);
    expect(upcoming(school(0), "2030-01-01")).toBe(true);
  });

  it("finds every word of the search in the name or address, without diacritics", () => {
    expect(matches(school(0), "zakladna skola")).toBe(true);
    expect(matches(school(0), "moyzesova 18")).toBe(true);
    expect(matches(school(1), "moyzesova")).toBe(false);
    expect(matches(school(1), "   ")).toBe(true);
  });

  it("orders events by their first day and the rest by name", () => {
    const sorted = [school(1), event(2), event(0), school(0)].sort(byOrder).map((p) => p.name);
    expect(sorted.slice(0, 2)).toEqual(["Radvanský jarmok", "Výstava fotografií"]);
    // An event without a day comes after those with one; an unnamed place after the named.
    const undated = { ...event(0), id: "u", startDate: null };
    expect(byOrder(undated, event(0))).toBeGreaterThan(0);
    const unnamed = { ...school(0), id: "n", name: null };
    expect(byOrder(unnamed, school(0))).toBeGreaterThan(0);
    expect(byOrder(school(0), unnamed)).toBeLessThan(0);
    expect(byOrder(unnamed, { ...unnamed, id: "m" })).toBe(0);
    expect(byOrder({ ...event(0), id: "a" }, event(0))).toBe(0);
  });

  it("draws only the places with a position, the picked one marked", () => {
    const features = featuresOf([event(0), event(2)], event(0).id).features;
    expect(features).toHaveLength(1);
    expect(features[0].properties).toMatchObject({ picked: true });
  });
});
