import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { byOrder, featuresOf, kindOf, matches, placeOf, safeUrl } from "./places";
import { answer, ISLES, POIS } from "./fixtures/praha";

const poi = (i: number) => toRichRow(POIS[i]);
const place = (i: number) => placeOf(poi(i), kindOf(poi(i), "PointOfInterest")!, "cs");

describe("kinds", () => {
  it("are the point of interest's category, and nothing for a category the map does not show", () => {
    expect([0, 1, 2, 3].map((i) => kindOf(poi(i), "PointOfInterest"))).toEqual(["school", "culture", "publicToilet", "ticketSale"]);
    expect(kindOf(poi(4), "PointOfInterest")).toBeNull();
    expect(kindOf(toRichRow(ISLES[0]), "WasteContainerIsle")).toBe("waste");
  });
});

describe("placeOf", () => {
  it("reads each kind's own fields and keeps a missing value missing", () => {
    expect(place(0)).toMatchObject({ name: "Základní škola Vodičkova", pupils: 410, facilityType: "základní škola", coordinates: [14.4232, 50.0807] });
    expect(place(2)).toMatchObject({ openingHours: "6:00–22:00", wheelchairAccessible: true });
    expect(place(3)).toMatchObject({ wheelchairAccessible: false, address: null, openingHours: null });
    expect(placeOf(toRichRow(ISLES[1]), "waste", "cs")).toMatchObject({ stationCode: "0002/ 004", access: "residents" });
  });

  it("never opens a link that is not http or https", () => {
    expect(place(5).url).toBeNull();
    expect(place(1).url).toBe("https://www.narodni-divadlo.cz/");
    expect(safeUrl("data:text/html,x")).toBeNull();
    expect(safeUrl("not a url")).toBeNull();
    expect(safeUrl(null)).toBeNull();
  });

  it("names a place in the reader's language, else Czech, else the first language there is", () => {
    const named = (languageMap: Record<string, string>) =>
      placeOf(toRichRow({ ...(POIS[0] as object), name: { type: "LanguageProperty", languageMap } } as never), "school", "en").name;
    expect(named({ en: "School", cs: "Škola" })).toBe("School");
    expect(named({ cs: "Škola", de: "Schule" })).toBe("Škola");
    expect(named({ de: "Schule" })).toBe("Schule");
    expect(named({ en: "  " })).toBeNull();
    // A plain text name, as a source without languages writes it.
    expect(placeOf(toRichRow({ ...(POIS[0] as object), name: { type: "Property", value: "Plain name" } } as never), "school", "en").name).toBe("Plain name");
  });
});

describe("search and order", () => {
  it("finds every word in the name, address, kind or point code, without diacritics", () => {
    expect(matches(place(1), "narodni divadlo")).toBe(true);
    expect(matches(place(0), "zakladni skola praha 1")).toBe(true);
    expect(matches(placeOf(toRichRow(ISLES[0]), "waste", "cs"), "0001")).toBe(true);
    expect(matches(place(2), "divadlo")).toBe(false);
  });

  it("orders by Czech name and draws only what has a position", () => {
    expect([place(2), place(1), place(0)].sort(byOrder).map((p) => p.name)).toEqual(["Národní divadlo", "Veřejné WC Anděl", "Základní škola Vodičkova"]);
    expect(featuresOf([place(1), place(5)], null).features).toHaveLength(1);
  });
});

describe("the fixture", () => {
  it("answers no rows for a type the map does not read", () => {
    expect(answer("Building")).toEqual([]);
    expect(answer(null)).toEqual([]);
  });
});

describe("the edges of a row", () => {
  const withCell = (attr: string, cell: unknown) => toRichRow({ ...(POIS[0] as object), [attr]: cell } as never);

  it("reads a number as text, a blank as missing, and keeps the first of several values", () => {
    expect(placeOf(withCell("address", { type: "Property", value: 12 }), "school", "cs").address).toBe("12");
    expect(placeOf(withCell("address", { type: "Property", value: "   " }), "school", "cs").address).toBeNull();
    expect(placeOf(withCell("address", { type: "Property", value: true }), "school", "cs").address).toBeNull();
    // T-4329: the city writes the Smart Data Models PostalAddress; it reads as one line.
    const postal = { streetAddress: "Na Příkopě 1", addressLocality: "Praha", addressCountry: "CZ" };
    expect(placeOf(withCell("address", { type: "Property", value: postal }), "school", "cs").address).toBe("Na Příkopě 1, Praha");
    expect(placeOf(withCell("address", { type: "Property", value: {} }), "school", "cs").address).toBeNull();
    expect(placeOf(withCell("address", [{ type: "Property", value: "Na Příkopě 1", datasetId: "urn:a" }, { type: "Property", value: "B", datasetId: "urn:b" }]), "school", "cs").address).toBe("Na Příkopě 1");
  });

  it("puts a place on the map only at a point on the globe", () => {
    const at = (value: unknown) => placeOf(withCell("location", { type: "GeoProperty", value }), "school", "cs").coordinates;
    expect(at({ type: "Point", coordinates: [14.4, 50.1] })).toEqual([14.4, 50.1]);
    expect(at({ type: "Point", coordinates: [190, 50] })).toBeNull();
    expect(at({ type: "Point", coordinates: [14.4, -91] })).toBeNull();
    expect(at({ type: "Point", coordinates: ["14", 50] })).toBeNull();
    expect(at({ type: "LineString", coordinates: [[14, 50]] })).toBeNull();
    expect(at({ type: "Point" })).toBeNull();
  });

  it("finds every place for an empty search, and orders unnamed places last", () => {
    expect(matches(place(0), "   ")).toBe(true);
    const unnamed = { ...place(0), name: null };
    expect([unnamed, place(1)].sort(byOrder).map((p) => p.name)).toEqual(["Národní divadlo", null]);
    expect(byOrder(unnamed, { ...unnamed })).toBe(0);
    expect(byOrder(place(1), unnamed)).toBe(-1);
  });
});
