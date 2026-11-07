import { describe, expect, it } from "vitest";
import { toRichCell, toRichRow, projectRow, attributesOf, cellText } from "../src/grid/model";
import { toRow } from "../src/ngsi";

describe("toRichCell", () => {
  it("returns null for non-object", () => {
    expect(toRichCell(null, "en")).toBeNull();
    expect(toRichCell("string", "en")).toBeNull();
    expect(toRichCell(42, "en")).toBeNull();
  });

  it("detects Property with value and metadata", () => {
    const cell = toRichCell(
      { type: "Property", value: 5, unitCode: "C62", observedAt: "2026-01-01T00:00:00Z", datasetId: "d1", createdAt: "2025-12-01T00:00:00Z", modifiedAt: "2026-01-02T00:00:00Z" },
      "en",
    );
    expect(cell).not.toBeNull();
    expect(cell!.kind).toBe("property");
    expect(cell!.value).toBe(5);
    expect(cell!.unitCode).toBe("C62");
    expect(cell!.observedAt).toBe("2026-01-01T00:00:00Z");
    expect(cell!.datasetId).toBe("d1");
    expect(cell!.createdAt).toBe("2025-12-01T00:00:00Z");
    expect(cell!.modifiedAt).toBe("2026-01-02T00:00:00Z");
  });

  it("unwraps DateTime in Property", () => {
    const cell = toRichCell(
      { type: "Property", value: { "@type": "DateTime", "@value": "2026-06-15T12:00:00Z" } },
      "en",
    );
    expect(cell!.value).toBe("2026-06-15T12:00:00Z");
  });

  it("detects Relationship with object", () => {
    const cell = toRichCell(
      { type: "Relationship", object: "urn:ngsi-ld:Device:example.org:demo:001" },
      "en",
    );
    expect(cell!.kind).toBe("relationship");
    expect(cell!.object).toBe("urn:ngsi-ld:Device:example.org:demo:001");
    expect(cell!.value).toBe("urn:ngsi-ld:Device:example.org:demo:001");
  });

  it("detects GeoProperty Point", () => {
    const cell = toRichCell(
      { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
      "en",
    );
    expect(cell!.kind).toBe("geo");
    expect(cell!.geometryType).toBe("Point");
    expect(cell!.value).toEqual({ type: "Point", coordinates: [24.93, 60.17] });
  });

  it("detects GeoProperty Polygon", () => {
    const cell = toRichCell(
      { type: "GeoProperty", value: { type: "Polygon", coordinates: [[[24.9, 60.1], [24.9, 60.2], [25.0, 60.2], [25.0, 60.1], [24.9, 60.1]]] } },
      "en",
    );
    expect(cell!.kind).toBe("geo");
    expect(cell!.geometryType).toBe("Polygon");
  });

  it("detects LanguageProperty with fallback", () => {
    const cell = toRichCell(
      { type: "LanguageProperty", languageMap: { fi: "Kaivopuisto", en: "Kaivopuisto Park" } },
      "fi",
    );
    expect(cell!.kind).toBe("language");
    expect(cell!.value).toBe("Kaivopuisto");
    expect(cell!.languageMap).toEqual({ fi: "Kaivopuisto", en: "Kaivopuisto Park" });

    // fallback to en
    const cellEn = toRichCell(
      { type: "LanguageProperty", languageMap: { fi: "Kaivopuisto", en: "Kaivopuisto Park" } },
      "sv",
    );
    expect(cellEn!.value).toBe("Kaivopuisto Park");

    // fallback to first
    const cellFirst = toRichCell(
      { type: "LanguageProperty", languageMap: { fi: "Kaivopuisto" } },
      "sv",
    );
    expect(cellFirst!.value).toBe("Kaivopuisto");
  });

  it("detects ListProperty", () => {
    const cell = toRichCell(
      { type: "ListProperty", valueList: ["a", "b", "c"] },
      "en",
    );
    expect(cell!.kind).toBe("list");
    expect(cell!.value).toEqual(["a", "b", "c"]);
  });

  it("detects JsonProperty", () => {
    const cell = toRichCell(
      { type: "JsonProperty", json: { nested: true } },
      "en",
    );
    expect(cell!.kind).toBe("json");
    expect(cell!.value).toEqual({ nested: true });
  });

  it("detects VocabProperty", () => {
    const cell = toRichCell(
      { type: "VocabProperty", vocab: "http://example.org/vocab#Bike" },
      "en",
    );
    expect(cell!.kind).toBe("vocab");
    expect(cell!.value).toBe("http://example.org/vocab#Bike");
  });

  it("detects VocabularyProperty as vocab", () => {
    const cell = toRichCell(
      { type: "VocabularyProperty", vocab: "http://example.org/vocab#Car" },
      "en",
    );
    expect(cell!.kind).toBe("vocab");
  });

  it("detects fallback when type missing but value present", () => {
    const cell = toRichCell({ value: 42 }, "en");
    expect(cell!.kind).toBe("property");
    expect(cell!.value).toBe(42);
  });

  it("detects fallback when type missing but object present", () => {
    const cell = toRichCell({ object: "urn:x" }, "en");
    expect(cell!.kind).toBe("relationship");
    expect(cell!.object).toBe("urn:x");
  });

  it("handles sub-properties", () => {
    const cell = toRichCell(
      {
        type: "Property",
        value: 10,
        subAttr: { type: "Property", value: "inner" },
      },
      "en",
    );
    expect(cell!.sub).toBeDefined();
    expect(cell!.sub!["subAttr"]).toBeDefined();
    const sub = cell!.sub!["subAttr"] as import("../src/grid/model").RichCell;
    expect(sub.kind).toBe("property");
    expect(sub.value).toBe("inner");
  });

  it("returns null for a non-attribute object", () => {
    expect(toRichCell({ random: true }, "en")).toBeNull();
  });
});

describe("toRichRow", () => {
  it("converts a full entity", () => {
    const entity = {
      id: "urn:ngsi-ld:Bike:example.org:demo:001",
      type: "Bike",
      createdAt: "2025-01-01T00:00:00Z",
      modifiedAt: "2025-06-01T00:00:00Z",
      scope: ["public"],
      "@context": "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context.jsonld",
      availableBikeNumber: {
        type: "Property",
        value: 5,
        unitCode: "C62",
        observedAt: "2026-01-01T00:00:00Z",
      },
      refDevice: {
        type: "Relationship",
        object: "urn:ngsi-ld:Device:example.org:demo:002",
      },
      location: {
        type: "GeoProperty",
        value: { type: "Point", coordinates: [24.93, 60.17] },
      },
    };

    const row = toRichRow(entity, "en");
    expect(row.id).toBe("urn:ngsi-ld:Bike:example.org:demo:001");
    expect(row.type).toBe("Bike");
    expect(row.createdAt).toBe("2025-01-01T00:00:00Z");
    expect(row.modifiedAt).toBe("2025-06-01T00:00:00Z");
    expect(row.scope).toEqual(["public"]);
    expect(row.cells["availableBikeNumber"]).toBeDefined();
    expect(row.cells["refDevice"]).toBeDefined();
    expect(row.cells["location"]).toBeDefined();
    expect(row.cells["@context"]).toBeUndefined();
    expect(row.raw).toBe(entity);
  });

  it("handles entity with no attributes", () => {
    const entity = { id: "urn:x", type: "T" };
    const row = toRichRow(entity);
    expect(Object.keys(row.cells)).toHaveLength(0);
  });

  it("handles null value", () => {
    const entity = { id: "urn:x", type: "T", attr: null };
    const row = toRichRow(entity);
    expect(row.cells["attr"]).toBeUndefined();
  });

  it("handles multi-instance attribute (array)", () => {
    const entity = {
      id: "urn:x",
      type: "T",
      multiAttr: [
        { type: "Property", value: 1, datasetId: "d1" },
        { type: "Property", value: 2, datasetId: "d2" },
      ],
    };
    const row = toRichRow(entity);
    const cells = row.cells["multiAttr"];
    expect(Array.isArray(cells)).toBe(true);
    expect(cells).toHaveLength(2);
    expect((cells as import("../src/grid/model").RichCell[])[0].value).toBe(1);
    expect((cells as import("../src/grid/model").RichCell[])[1].value).toBe(2);
  });

  it("single-instance array stays as single RichCell", () => {
    const entity = {
      id: "urn:x",
      type: "T",
      singleAttr: [{ type: "Property", value: 1 }],
    };
    const row = toRichRow(entity);
    const cell = row.cells["singleAttr"];
    expect(Array.isArray(cell)).toBe(false);
    expect((cell as import("../src/grid/model").RichCell).value).toBe(1);
  });
});

describe("projectRow", () => {
  it("equals toRow for the same raw entity", () => {
    const entity = {
      id: "urn:ngsi-ld:Bike:example.org:demo:001",
      type: "Bike",
      availableBikeNumber: { type: "Property", value: 5, unitCode: "C62" },
      refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:example.org:demo:002" },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
    };
    const rich = toRichRow(entity);
    const projected = projectRow(rich);
    const expected = toRow(entity);
    expect(projected).toEqual(expected);
  });

  it("works with empty entity", () => {
    const entity = { id: "", type: "" };
    const rich = toRichRow(entity);
    const projected = projectRow(rich);
    expect(projected).toEqual({ id: "", type: "" });
  });
});

describe("attributesOf", () => {
  it("returns union in first-seen order", () => {
    const rows = [
      toRichRow({ id: "1", type: "T", a: { type: "Property", value: 1 }, b: { type: "Property", value: 2 } }),
      toRichRow({ id: "2", type: "T", c: { type: "Property", value: 3 }, a: { type: "Property", value: 4 } }),
    ];
    expect(attributesOf(rows)).toEqual(["a", "b", "c"]);
  });

  it("returns empty for no rows", () => {
    expect(attributesOf([])).toEqual([]);
  });
});

describe("cellText", () => {
  it("returns empty for undefined/null", () => {
    expect(cellText(undefined)).toBe("");
    expect(cellText(null as unknown as import("../src/grid/model").RichCell)).toBe("");
  });

  it("returns property value as string", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "property", value: 42 };
    expect(cellText(cell)).toBe("42");
  });

  it("returns a PostalAddress as its line and any other object as JSON (T-3949)", () => {
    const address: import("../src/grid/model").RichCell = {
      kind: "property",
      value: { streetAddress: "Siltakatu 11", postalCode: "00580", addressLocality: "Helsinki", addressCountry: "FI" },
    };
    expect(cellText(address)).toBe("Siltakatu 11, 00580 Helsinki");
    const other: import("../src/grid/model").RichCell = { kind: "property", value: { a: 1 } };
    expect(cellText(other)).toBe('{"a":1}');
  });

  it("returns relationship object", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "relationship", value: "urn:x", object: "urn:x" };
    expect(cellText(cell)).toBe("urn:x");
  });

  it("returns geo geometryType", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "geo", value: {}, geometryType: "Point" };
    expect(cellText(cell)).toBe("Point");
  });

  it("returns language value", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "language", value: "Kaivopuisto", languageMap: {} };
    expect(cellText(cell)).toBe("Kaivopuisto");
  });

  it("returns list joined by comma", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "list", value: ["a", "b", "c"] };
    expect(cellText(cell)).toBe("a, b, c");
  });

  it("returns json stringified", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "json", value: { nested: true } };
    expect(cellText(cell)).toBe('{"nested":true}');
  });

  it("returns vocab value", () => {
    const cell: import("../src/grid/model").RichCell = { kind: "vocab", value: "http://example.org/vocab#Bike" };
    expect(cellText(cell)).toBe("http://example.org/vocab#Bike");
  });

  it("joins array cells with |", () => {
    const cells: import("../src/grid/model").RichCell[] = [
      { kind: "property", value: 1 },
      { kind: "property", value: 2 },
    ];
    expect(cellText(cells)).toBe("1 | 2");
  });
});
