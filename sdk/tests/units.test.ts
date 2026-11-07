/**
 * T-2812 (DM-06): a value written with its unit's symbol from the platform's code list, the
 * number in the reader's locale.
 */
import { describe, expect, it } from "vitest";
import { formatValue, unitSymbol, unitTitle } from "../src/sdk";

describe("formatValue", () => {
  it("writes the number per locale and the symbol from the list", () => {
    expect(formatValue(23.4, "GQ", "en")).toBe("23.4 µg/m³");
    expect(formatValue(23.4, "GQ", "sk")).toBe("23,4 µg/m³");
    expect(formatValue(1234.5, "CEL", "de")).toBe("1.234,5 °C");
    expect(formatValue(0.001234567, "P1", "en")).toBe("0.001235 %");
  });

  it("writes a number alone where the unit has no symbol or is the dimensionless one", () => {
    expect(formatValue(3, "C62", "en")).toBe("3");
    expect(formatValue(3, "H87", "en")).toBe("3");
    expect(formatValue(3, undefined, "en")).toBe("3");
  });

  it("keeps a code the list does not know, and writes what is not a number as it is", () => {
    expect(formatValue(3, "XQZ", "en")).toBe("3 XQZ");
    expect(formatValue("n/a", "GQ", "en")).toBe("n/a µg/m³");
    expect(formatValue({ a: 1 }, undefined, "en")).toBe('{"a":1}');
    expect(formatValue(null, "GQ", "en")).toBe("");
    expect(formatValue(Number.NaN, "GQ", "en")).toBe("—");
  });
});

describe("unitSymbol and unitTitle", () => {
  it("name the unit for a reader and for a hover", () => {
    expect(unitSymbol("A97")).toBe("hPa");
    expect(unitSymbol("")).toBe("");
    expect(unitTitle("GQ")).toBe("microgram per cubic metre (GQ)");
    expect(unitTitle("XQZ")).toBe("XQZ");
    expect(unitTitle(undefined)).toBe("");
  });
});
