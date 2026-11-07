/** T-4264, T-4160 #13: an area is the region's name, never a NUTS code or its URI. */
import { describe, expect, it } from "vitest";
import { areaLabel, nutsCode } from "../src/pages/catalogue/nuts";

describe("area names (T-4264)", () => {
  it("names a NUTS region by its code or its EU URI", () => {
    expect(areaLabel("SK032")).toBe("Banskobystrický kraj");
    expect(areaLabel("http://data.europa.eu/nuts/code/CZ010")).toBe("Hlavní město Praha");
    expect(areaLabel("fi1b1")).toBe("Helsinki-Uusimaa");
    expect(nutsCode("http://data.europa.eu/nuts/code/SK031")).toBe("SK031");
  });

  it("shows any other place as it came", () => {
    expect(areaLabel("DE300")).toBe("DE300");
    expect(areaLabel("https://www.geonames.org/3060972")).toBe("https://www.geonames.org/3060972");
    expect(areaLabel("")).toBe("");
  });
});
