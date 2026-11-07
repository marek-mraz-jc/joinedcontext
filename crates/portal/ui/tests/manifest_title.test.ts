import { describe, expect, it } from "vitest";
import { localized, plainTitle } from "../src/api/manifest";
import { contextSpaceSchema, dashboardSchema, endpointSchema } from "../src/schemas/kinds";

describe("a manifest title (UI-50)", () => {
  it("reads a plain string as it is, whatever the locale", () => {
    expect(localized("City bikes", "sk", "city-bikes")).toBe("City bikes");
  });

  it("falls back to the name for an empty or missing title", () => {
    expect(localized("  ", "en", "city-bikes")).toBe("city-bikes");
    expect(localized(undefined, "en", "city-bikes")).toBe("city-bikes");
  });

  it("still reads the legacy language map", () => {
    expect(localized({ sk: "Bicykle", en: "Bikes" }, "sk-SK", "bikes")).toBe("Bicykle");
    expect(localized({ de: "Räder" }, "cs", "bikes")).toBe("bikes");
  });

  it("gives a form the one string it edits: the plain text, or the legacy map's en, then its first text", () => {
    expect(plainTitle("City bikes")).toBe("City bikes");
    expect(plainTitle({ sk: "Bicykle", en: "Bikes" })).toBe("Bikes");
    expect(plainTitle({ sk: "Bicykle", en: "" })).toBe("Bicykle");
    expect(plainTitle({ en: " " })).toBeUndefined();
    expect(plainTitle("")).toBeUndefined();
    expect(plainTitle(undefined)).toBeUndefined();
    expect(plainTitle({ en: 7 })).toBeUndefined();
  });

  it("asks for a title in one box, never one per language", () => {
    const t = (key: string) => key;
    for (const schema of [contextSpaceSchema(t), dashboardSchema(t, []), endpointSchema(t, [])]) {
      expect(schema.properties?.title).toEqual({ type: "string", title: expect.any(String) });
    }
  });
});
