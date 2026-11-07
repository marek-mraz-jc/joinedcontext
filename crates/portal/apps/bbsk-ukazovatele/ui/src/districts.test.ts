/**
 * The join of the register's district outlines to an indicator's bars (T-2933).
 */
import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { choropleth, districtsOf, rampColor, rangeOf, territoryOf } from "./districts";
import { LOCALES } from "./locales";

const SQUARE = {
  type: "Polygon",
  coordinates: [
    [
      [19.1, 48.7],
      [19.2, 48.7],
      [19.2, 48.8],
      [19.1, 48.7],
    ],
  ],
};

const area = (localId: string, attrs: Record<string, unknown>) =>
  toRichRow(
    {
      id: `urn:ngsi-ld:AdministrativeArea:bbsk.sk:bbsk-registre:${localId}`,
      type: "AdministrativeArea",
      ...attrs,
    },
    "sk",
  );

const district = (localId: string, name: string, location: unknown = SQUARE) =>
  area(localId, {
    name: { type: "LanguageProperty", languageMap: { sk: name } },
    divisionLevel: { type: "Property", value: "district" },
    location: { type: "GeoProperty", value: location },
  });

describe("the territory of a district's name", () => {
  it("gives every district the locale names its own territory token", () => {
    const districts = Object.entries(LOCALES.sk.territory).filter(([key]) => key.startsWith("okres-"));
    expect(districts.length).toBeGreaterThan(10);
    for (const [key, name] of districts) {
      expect(territoryOf(name)).toBe(key);
    }
  });

  it("reads the register's spelling, diacritics and all, and refuses an empty name", () => {
    expect(territoryOf("Okres Veľký Krtíš")).toBe("okres-velky-krtis");
    expect(territoryOf("Okres Žiar nad Hronom")).toBe("okres-ziar-nad-hronom");
    expect(territoryOf("Okres ")).toBeNull();
    expect(territoryOf("")).toBeNull();
  });
});

describe("the district shapes among the register's rows", () => {
  it("keeps districts with an area outline and leaves out the rest", () => {
    const rows = [
      district("SK0321", "Okres Brezno"),
      // A municipality, a point, a nameless district and another type are no district shape.
      area("SK0321508438", {
        name: { type: "LanguageProperty", languageMap: { sk: "Brezno" } },
        divisionLevel: { type: "Property", value: "municipality" },
        location: { type: "GeoProperty", value: SQUARE },
      }),
      district("SK0322", "Okres Detva", { type: "Point", coordinates: [19.4, 48.5] }),
      area("SK0323", {
        divisionLevel: { type: "Property", value: "district" },
        location: { type: "GeoProperty", value: SQUARE },
      }),
      toRichRow({ id: "urn:ngsi-ld:Hospital:bbsk.sk:bbsk-registre:1", type: "Hospital" }, "sk"),
    ];
    expect(districtsOf(rows)).toEqual([{ territory: "okres-brezno", geometry: SQUARE }]);
  });
});

describe("the map of one indicator", () => {
  const districts = [
    { territory: "okres-brezno", geometry: SQUARE },
    { territory: "okres-detva", geometry: SQUARE },
    { territory: "okres-poltar", geometry: SQUARE },
  ] as Parameters<typeof choropleth>[0];

  it("gives one feature per district, its number where it has one, and the range of those", () => {
    const { features, ramp } = choropleth(districts, [
      { territory: "okres-detva", value: 30199 },
      { territory: "okres-brezno", value: 57517 },
      // A bar with no outline colours nothing.
      { territory: "okres-krupina", value: 21129 },
    ]);
    expect(features.map((feature) => [feature.id, feature.properties])).toEqual([
      ["okres-brezno", { value: 57517 }],
      ["okres-detva", { value: 30199 }],
      ["okres-poltar", {}],
    ]);
    expect(ramp).toEqual([30199, 57517]);
  });

  it("has no range when no district is measured", () => {
    expect(choropleth(districts, []).ramp).toBeNull();
    expect(choropleth([], [{ territory: "okres-brezno", value: 1 }])).toEqual({ features: [], ramp: null });
  });
});

describe("a value's colour on the map's scale", () => {
  const colors = { low: "#0284c7", high: "#ea580c" };

  it("runs from low at the range's bottom to high at its top, as the map's linear fill", () => {
    expect(rampColor(10, [10, 30], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 0%)");
    expect(rampColor(20, [10, 30], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 50%)");
    expect(rampColor(30, [10, 30], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 100%)");
    expect(rampColor(0.33, [0.3, 0.7], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 7.5%)");
  });

  it("holds a value outside the range at its end, and one value alone at the low end", () => {
    expect(rampColor(-5, [10, 30], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 0%)");
    expect(rampColor(99, [10, 30], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 100%)");
    expect(rampColor(4, [4, 4], colors)).toBe("color-mix(in srgb, #0284c7, #ea580c 0%)");
  });

  it("has a range only where there is a value", () => {
    expect(rangeOf([])).toBeNull();
    expect(rangeOf([3, -1, 2])).toEqual([-1, 3]);
  });
});

describe("a register row's odd shapes", () => {
  it("takes the first of several names", () => {
    const row = area("SK0322", {
      name: [
        { type: "LanguageProperty", languageMap: { sk: "Okres Brezno" }, datasetId: "urn:a" },
        { type: "LanguageProperty", languageMap: { sk: "Brezno" }, datasetId: "urn:b" },
      ],
      divisionLevel: { type: "Property", value: "district" },
      location: { type: "GeoProperty", value: SQUARE },
    });
    expect(districtsOf([row]).map((district) => district.territory)).toEqual(["okres-brezno"]);
  });
});
