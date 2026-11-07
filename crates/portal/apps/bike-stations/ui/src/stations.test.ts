import { describe, expect, it } from "vitest";
import type { RichRow } from "@joinedcontext/sdk";
import { byName, folded, matches, standingOf, stationOf, totals } from "./stations";
import { answer } from "./fixtures/bikes";
import type { Station } from "./stations";

const row = (cells: Record<string, unknown>): RichRow =>
  ({ id: "urn:ngsi-ld:BikeHireDockingStation:x", type: "BikeHireDockingStation", cells }) as unknown as RichRow;
const station = (bikes: number | null, docks: number | null): Station => ({
  id: "x",
  name: "Kaivopuisto",
  coordinates: null,
  bikes,
  docks,
});

describe("a station", () => {
  it("keeps a count only when it is a whole number of zero or more", () => {
    for (const bad of [-1, 2.5, "7", null, Number.NaN]) {
      expect(stationOf(row({ availableBikeNumber: { value: bad } }), "en").bikes).toBeNull();
    }
    expect(stationOf(row({ availableBikeNumber: { value: 0 } }), "en").bikes).toBe(0);
  });

  it("drops a position outside the globe and reads a language map in the reader's language", () => {
    const far = stationOf(row({ location: { value: { type: "Point", coordinates: [200, 60] } } }), "en");
    expect(far.coordinates).toBeNull();
    const named = stationOf(row({ name: { languageMap: { fi: "Rautatientori", sv: "Järnvägstorget" } } }), "sv");
    expect(named.name).toBe("Järnvägstorget");
  });

  it("reads a name in the reader's language, else Finnish, else English, else the one written; a blank or odd value as none", () => {
    const named = (cell: unknown, locale = "sv") => stationOf(row({ name: cell }), locale).name;
    expect(named({ languageMap: { fi: "Rautatientori", en: "Railway Square" } })).toBe("Rautatientori");
    expect(named({ languageMap: { en: "Railway Square" } })).toBe("Railway Square");
    expect(named({ languageMap: { de: "Bahnhofsplatz" } })).toBe("Bahnhofsplatz");
    expect(named({ languageMap: { sv: "  " } })).toBeNull();
    expect(named({ value: " Kamppi " })).toBe("Kamppi");
    expect(named({ value: "" })).toBeNull();
    expect(named({ value: 7 })).toBeNull();
    // Several values of one attribute: the first is the one read.
    expect(named([{ value: "Kamppi" }, { value: "Kampen" }])).toBe("Kamppi");
  });

  it("puts a station without a name last, two of them side by side", () => {
    const unnamed = { ...station(1, 1), id: "u", name: null };
    const other = { ...station(1, 1), id: "v", name: null };
    expect([unnamed, station(1, 1)].sort(byName).map((one) => one.id)).toEqual(["x", "u"]);
    expect([station(1, 1), unnamed].sort(byName).map((one) => one.id)).toEqual(["x", "u"]);
    expect(byName(unnamed, other)).toBe(0);
    expect(answer("Other")).toEqual([]);
  });

  it("is empty with no bike, full with no dock, unknown when it reports neither", () => {
    expect(standingOf(station(0, 5))).toBe("empty");
    expect(standingOf(station(5, 0))).toBe("full");
    expect(standingOf(station(null, null))).toBe("unknown");
    expect(standingOf(station(2, 3))).toBe("available");
    // No bike and no dock reads as empty: there is nothing to take, which is what a rider asks.
    expect(standingOf(station(0, 0))).toBe("empty");
  });
});

describe("the stations together", () => {
  it("add only what each reports", () => {
    expect(totals([station(3, null), station(null, 4), station(null, null)])).toEqual({ bikes: 3, docks: 4 });
    expect(totals([])).toEqual({ bikes: 0, docks: 0 });
  });

  it("match every word of a search, without diacritics", () => {
    expect(folded("Töölö")).toBe("toolo");
    const named = { ...station(1, 1), name: "Töölönlahdenkatu" };
    expect(matches(named, "TOOLON lahden")).toBe(true);
    expect(matches(named, "toolon kallio")).toBe(false);
    expect(matches({ ...named, name: null }, "a")).toBe(false);
    expect(matches(named, "   ")).toBe(true);
  });
});
