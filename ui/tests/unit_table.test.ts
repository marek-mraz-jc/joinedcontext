/**
 * The unit code list and its search (DM-06, DM-59, T-2809): the whole of UNECE Recommendation 20
 * joined with QUDT, generated in the platform repository and copied here byte for byte.
 */
import { describe, expect, it } from "vitest";
import { UNITS, conversion, convertible, searchUnits, unitLabel, unitOf } from "../src/units";

describe("the code list", () => {
  it("is the whole recommendation, sorted and unique, with the frequent set in it", () => {
    expect(UNITS.length).toBeGreaterThan(1500);
    const codes = UNITS.map((unit) => unit.code);
    expect(new Set(codes).size).toBe(codes.length);
    expect([...codes].sort()).toEqual(codes);
    for (const code of ["GQ", "CEL", "P1", "KMH", "KWH", "A97", "C62"]) {
      expect(unitOf(code)?.frequent, code).toBe(true);
    }
  });

  it("knows hectopascal as A97, and HPA as the alcohol it is", () => {
    expect(unitOf("A97")?.name).toBe("hectopascal");
    expect(unitOf("HPA")?.name).toBe("hectolitre of pure alcohol");
    expect(unitOf("HPA")?.frequent).toBe(false);
  });

  it("finds a code exactly and nothing else by case or a stranger's spelling", () => {
    expect(unitOf("GQ")?.symbol).toBe("µg/m³");
    expect(unitOf("gq")).toBeUndefined();
    expect(unitOf("ug/m3")).toBeUndefined();
    expect(unitOf(undefined)).toBeUndefined();
  });

  it("labels a unit by its symbol and name, and by the name alone where it has no symbol", () => {
    expect(unitLabel(unitOf("GQ")!)).toBe("µg/m³ — microgram per cubic metre");
    expect(unitLabel(unitOf("H87")!)).toBe("piece");
  });
});

describe("searchUnits", () => {
  it.each(["µg", "microgram", "GQ", "ug/m3", "µg/m³", "ug.m-3"])("finds GQ for %s", (typed) => {
    expect(searchUnits(typed).units.map((unit) => unit.code)).toContain("GQ");
  });

  it("puts the exact code first and the frequent units before the rest", () => {
    expect(searchUnits("cel").units[0].code).toBe("CEL");
    const metres = searchUnits("metre").units;
    const firstRare = metres.findIndex((unit) => !unit.frequent);
    expect(firstRare).toBeGreaterThan(0);
    expect(metres.slice(firstRare).every((unit) => !unit.frequent)).toBe(true);
  });

  it("groups the rest by quantity kind, each kind in one run", () => {
    const kinds = searchUnits("per").units.filter((unit) => !unit.frequent).map((unit) => unit.quantityKinds[0] ?? "");
    const runs = kinds.filter((kind, index) => index === 0 || kinds[index - 1] !== kind);
    expect(new Set(runs).size).toBe(runs.length);
  });

  it("answers the frequent set for empty text and caps a broad search with its total", () => {
    const empty = searchUnits("  ");
    expect(empty.units.length).toBe(empty.total);
    expect(empty.units.every((unit) => unit.frequent)).toBe(true);
    const broad = searchUnits("e", 20);
    expect(broad.units).toHaveLength(20);
    expect(broad.total).toBeGreaterThan(20);
  });

  it("finds a unit by its quantity kind, and nothing for nonsense", () => {
    expect(searchUnits("temperature").units.map((unit) => unit.code)).toEqual(expect.arrayContaining(["CEL", "KEL", "FAH"]));
    expect(searchUnits("zzqx").units).toEqual([]);
  });

  it("offers a deprecated code only to someone who types it", () => {
    const deprecated = UNITS.find((unit) => unit.deprecated);
    expect(deprecated).toBeDefined();
    expect(searchUnits(deprecated!.name).units).not.toContain(deprecated);
    expect(searchUnits(deprecated!.code).units[0]).toBe(deprecated);
  });
});

describe("conversion", () => {
  const unit = (code: string) => unitOf(code)!;

  it("reads the factor and offset from the list", () => {
    expect(conversion(unit("GP"), unit("GQ"))).toEqual({ factor: 1000, offset: 0 });
    expect(conversion(unit("KMH"), unit("MTS"))).toEqual({ factor: 0.277777777778, offset: 0 });
    const fahrenheit = conversion(unit("FAH"), unit("CEL"))!;
    expect(212 * fahrenheit.factor + fahrenheit.offset).toBeCloseTo(100, 9);
    expect(32 * fahrenheit.factor + fahrenheit.offset).toBeCloseTo(0, 9);
    expect(conversion(unit("CEL"), unit("CEL"))).toEqual({ factor: 1, offset: 0 });
  });

  it("refuses units of different quantities and units without a factor", () => {
    expect(conversion(unit("CEL"), unit("GQ"))).toBeUndefined();
    expect(conversion(unit("D61"), unit("P1"))).toBeUndefined();
    expect(conversion(unit("2N"), unit("P1"))).toBeUndefined();
    expect(convertible(unit("H87"), unit("H87"))).toBe(true);
  });
});
