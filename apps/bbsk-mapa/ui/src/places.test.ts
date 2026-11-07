import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { byOrder, featuresOf, matches, placeOf } from "./places";
import { answer, HOSPITALS, ORGANIZATIONS, SOCIAL } from "./fixtures/registre";

const hospital = (i: number) => placeOf(toRichRow(HOSPITALS[i]), "hospital", "sk");
const social = (i: number) => placeOf(toRichRow(SOCIAL[i]), "social", "sk");
const organization = (i: number) => placeOf(toRichRow(ORGANIZATIONS[i]), "organization", "sk");

describe("placeOf", () => {
  it("reads a place's name in the reader's language, its position, district and kind of service", () => {
    expect(hospital(0)).toMatchObject({ name: "Fakultná nemocnica s poliklinikou F. D. Roosevelta", coordinates: [19.1386, 48.7432] });
    expect(social(0)).toMatchObject({ district: "Rimavská Sobota", serviceKind: "domov sociálnych služieb" });
    // T-4329: the register writes the Smart Data Models PostalAddress; it reads as one line.
    expect(hospital(1).address).toBe("Kuzmányho nábrežie 28, Zvolen");
    const at = (value: unknown) => placeOf(toRichRow({ id: "urn:ngsi-ld:PointOfInterest:x", type: "PointOfInterest", address: { type: "Property", value } }), "hospital", "sk").address;
    expect(at({})).toBeNull();
    expect(at(["a"])).toBeNull();
  });

  it("reads a name in the reader's language, else Slovak, else the one written, and a plain text name too", () => {
    const named = (name: unknown) => placeOf(toRichRow({ id: "urn:ngsi-ld:PointOfInterest:x", type: "PointOfInterest", name }), "hospital", "en").name;
    expect(named({ type: "LanguageProperty", languageMap: { sk: "Nemocnica", en: "Hospital" } })).toBe("Hospital");
    expect(named({ type: "LanguageProperty", languageMap: { sk: "Nemocnica" } })).toBe("Nemocnica");
    expect(named({ type: "LanguageProperty", languageMap: { de: "Krankenhaus" } })).toBe("Krankenhaus");
    expect(named({ type: "LanguageProperty", languageMap: { en: "  " } })).toBeNull();
    expect(named({ type: "Property", value: " Poliklinika " })).toBe("Poliklinika");
    expect(named({ type: "Property", value: "" })).toBeNull();
    expect(named({ type: "Property", value: 7 })).toBeNull();
  });

  it("draws only a point inside the world", () => {
    const at = (value: unknown) => placeOf(toRichRow({ id: "urn:ngsi-ld:PointOfInterest:x", type: "PointOfInterest", location: { type: "GeoProperty", value } }), "hospital", "sk").coordinates;
    expect(at({ type: "Point", coordinates: [19.1, 48.7] })).toEqual([19.1, 48.7]);
    expect(at({ type: "Point", coordinates: [190, 48.7] })).toBeNull();
    expect(at({ type: "Point", coordinates: [19.1, -91] })).toBeNull();
    expect(at({ type: "Point", coordinates: ["19", 48.7] })).toBeNull();
    expect(at({ type: "Polygon", coordinates: [] })).toBeNull();
  });

  it("keeps a value the register does not publish missing", () => {
    expect(social(1)).toMatchObject({ serviceKind: null, coordinates: null });
    expect(hospital(1)).toMatchObject({ district: null });
  });
});

describe("search and order", () => {
  it("finds words in the name, address, district or kind of service, without diacritics", () => {
    expect(matches(social(0), "rimavska")).toBe(true);
    expect(matches(social(0), "domov socialnych")).toBe(true);
    expect(matches(organization(1), "muzeum")).toBe(false);
  });

  it("orders by name and draws only what has a position", () => {
    const sorted = [organization(1), hospital(1), organization(0)].sort(byOrder).map((p) => p.name);
    expect(sorted).toEqual(["Nemocnica Zvolen", "Spojená škola Detva", "Stredoslovenské múzeum"]);
    expect(featuresOf([hospital(0), social(0), organization(1)], null).features).toHaveLength(1);
  });

  it("puts a place without a name last, and two of them side by side", () => {
    const unnamed = { ...hospital(1), id: "u1", name: null };
    const other = { ...hospital(1), id: "u2", name: null };
    expect([unnamed, hospital(1)].sort(byOrder).map((p) => p.id)).toEqual([hospital(1).id, "u1"]);
    expect([hospital(1), unnamed].sort(byOrder).map((p) => p.id)).toEqual([hospital(1).id, "u1"]);
    expect(byOrder(unnamed, other)).toBe(0);
  });

  it("marks the picked place, and the fixtures answer nothing for a type the space does not hold", () => {
    const [drawn] = featuresOf([hospital(0)], hospital(0).id).features;
    expect(drawn.properties.picked).toBe(true);
    expect(answer("Bridge")).toEqual([]);
  });
});
