import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { byOrder, featuresOf, kindOf, matches, placeOf } from "./places";
import { SERVICES, WATER } from "./fixtures/helsinki";

const service = (i: number, locale = "fi") => {
  const row = toRichRow(SERVICES[i]);
  return placeOf(row, kindOf(row, "PointOfInterest")!, locale);
};
const sensor = (i: number) => placeOf(toRichRow(WATER[i]), "water", "fi");

describe("kinds", () => {
  it("are the register's category, nothing for one the map does not show, and water for a sensor", () => {
    expect([0, 1, 2, 3, 4].map((i) => kindOf(toRichRow(SERVICES[i]), "PointOfInterest"))).toEqual(["library", "healthStation", "swimmingHall", "beach", "school"]);
    expect(kindOf(toRichRow(SERVICES[5]), "PointOfInterest")).toBeNull();
    expect(kindOf(toRichRow(WATER[0]), "WaterQualityObserved")).toBe("water");
  });
});

describe("placeOf", () => {
  it("reads the name in the reader's language, else Finnish", () => {
    expect(service(0, "en").name).toBe("Central Library Oodi");
    expect(service(0, "sv").name).toBe("Centrumbiblioteket Ode");
    expect(service(1, "en").name).toBe("Kallion terveysasema");
  });

  it("reads a sensor's temperature, and keeps a missing reading missing", () => {
    expect(sensor(0).temperature).toBe(16.4);
    expect(sensor(1).temperature).toBeNull();
  });
});

describe("search and order", () => {
  it("finds every word in the name or address, without diacritics", () => {
    expect(matches(service(2), "yrjonkadun uimahalli")).toBe(true);
    expect(matches(service(0), "toolonlahdenkatu")).toBe(true);
    expect(matches(service(1), "oodi")).toBe(false);
  });

  it("orders by Finnish name and draws only what has a position", () => {
    expect([service(2), service(0), service(1)].sort(byOrder).map((p) => p.name)).toEqual(["Kallion terveysasema", "Keskustakirjasto Oodi", "Yrjönkadun uimahalli"]);
    expect(featuresOf([service(0), service(4)], null).features).toHaveLength(1);
  });
});

describe("what a register may send that the map still reads", () => {
  const place = (entity: Record<string, unknown>, locale = "fi") => {
    const row = toRichRow({ id: "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:x", type: "PointOfInterest", serviceCategory: { type: "Property", value: "library" }, ...entity });
    return placeOf(row, "library", locale);
  };

  it("names a place in the reader's language, else Finnish, else English, else any; a blank name is no name", () => {
    const names = { fi: "Kirjasto", en: "Library", sv: "Bibliotek" };
    expect(place({ name: { type: "LanguageProperty", languageMap: names } }, "sv").name).toBe("Bibliotek");
    expect(place({ name: { type: "LanguageProperty", languageMap: { en: "Library", sv: "Bibliotek" } } }, "de").name).toBe("Library");
    expect(place({ name: { type: "LanguageProperty", languageMap: { sv: "Bibliotek" } } }, "de").name).toBe("Bibliotek");
    expect(place({ name: { type: "LanguageProperty", languageMap: { fi: "  " } } }).name).toBeNull();
    expect(place({ name: { type: "Property", value: " Kirjasto " } }).name).toBe("Kirjasto");
    expect(place({ name: { type: "Property", value: "" } }).name).toBeNull();
    expect(place({ name: { type: "Property", value: 42 } }).name).toBe("42");
    expect(place({}).name).toBeNull();
  });

  it("reads the first of several values, and a temperature that is no number as none", () => {
    expect(place({ address: [{ type: "Property", value: "Töölönlahdenkatu 4", datasetId: "urn:a" }, { type: "Property", value: "B", datasetId: "urn:b" }] }).address).toBe("Töölönlahdenkatu 4");
    // T-3949: the register's address is a PostalAddress, shown as its line.
    expect(
      place({ address: { type: "Property", value: { streetAddress: "Töölönlahdenkatu 4", postalCode: "00100", addressLocality: "Helsinki", addressCountry: "FI" } } })
        .address,
    ).toBe("Töölönlahdenkatu 4, 00100 Helsinki");
    expect(place({ temperature: { type: "Property", value: "warm" } }).temperature).toBeNull();
  });

  it("places only a point on the globe", () => {
    expect(place({ location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.9, 60.2] } } }).coordinates).toEqual([24.9, 60.2]);
    expect(place({ location: { type: "GeoProperty", value: { type: "Point", coordinates: [200, 60] } } }).coordinates).toBeNull();
    expect(place({ location: { type: "GeoProperty", value: { type: "Polygon", coordinates: [] } } }).coordinates).toBeNull();
    expect(place({ location: { type: "GeoProperty", value: { type: "Point", coordinates: ["x", 1] } } }).coordinates).toBeNull();
  });

  it("knows no kind for a service with no category", () => {
    expect(kindOf(toRichRow({ id: "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:y", type: "PointOfInterest" }), "PointOfInterest")).toBeNull();
  });

  it("finds by an empty search, and searches an unnamed place without an address", () => {
    const nameless = place({});
    expect(matches(nameless, "")).toBe(true);
    expect(matches(nameless, "oodi")).toBe(false);
  });

  it("orders the unnamed last", () => {
    const named = place({ name: { type: "Property", value: "A" } });
    const nameless = place({});
    expect([nameless, named, place({})].sort(byOrder).map((p) => p.name)).toEqual(["A", null, null]);
  });
});
