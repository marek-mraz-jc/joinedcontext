import { describe, expect, it } from "vitest";
import { aggregate, applyFilters, cell, columnKind, displayName, entitiesUrl, extent, format, pointOf, toRow } from "../src/ngsi";
import type { Cell, Row } from "../src/ngsi";

const rows: Row[] = [
  { id: "urn:a", type: "T", name: "Kamppi", bikes: 12, seen: "2026-09-12T10:00:00Z", location: { type: "Point", coordinates: [24.93, 60.17] } },
  { id: "urn:b", type: "T", name: "Kallio", bikes: 3, seen: "2026-09-12T11:00:00Z", location: { type: "Point", coordinates: [24.95, 60.18] } },
  { id: "urn:c", type: "T", name: "Pasila", bikes: null, seen: null, location: null },
];

describe("cell", () => {
  it("reads a keyValues LanguageProperty as its English or first text", () => {
    expect(cell({ languageMap: { fi: "Kaivopuisto" } })).toBe("Kaivopuisto");
    expect(cell({ languageMap: { fi: "Kaivopuisto", en: "Kaivopuisto Park" } })).toBe("Kaivopuisto Park");
  });
  it("unwraps the broker's keyValues shapes", () => {
    expect(cell({ "@type": "DateTime", "@value": "2026-01-01T00:00:00Z" })).toBe("2026-01-01T00:00:00Z");
    expect(cell({ en: "Bikes", fi: "Pyörät" })).toBe("Bikes");
    expect(cell({ type: "Point", coordinates: [1, 2] })).toEqual({ type: "Point", coordinates: [1, 2] });
    expect(cell({ object: "urn:x" })).toBe("urn:x");
    expect(cell(["a", 1])).toBe("a, 1");
    expect(cell(undefined)).toBeNull();
  });
  // T-4422: the entity panel reads its row through toRow; a PostalAddress was its first string.
  it("reads a PostalAddress as its one line, a Property's or keyValues", () => {
    const address = { streetAddress: "Kuzmányho nábrežie 28", addressLocality: "Zvolen" };
    expect(cell({ type: "Property", value: address })).toBe("Kuzmányho nábrežie 28, Zvolen");
    expect(cell(address)).toBe("Kuzmányho nábrežie 28, Zvolen");
    expect(cell({ label: "x", other: "y" })).toBe("x");
  });
  it("keeps id and type whatever the entity carries", () => {
    expect(toRow({ id: "urn:x", type: "T", "@context": [], n: 1 })).toEqual({ id: "urn:x", type: "T", n: 1 });
    expect(toRow({}).id).toBe("");
  });
});

describe("columns", () => {
  it("classifies by the values present, nulls ignored", () => {
    expect(columnKind(rows, "bikes")).toBe("number");
    expect(columnKind(rows, "seen")).toBe("date");
    expect(columnKind(rows, "location")).toBe("geo");
    expect(columnKind(rows, "name")).toBe("text");
    expect(columnKind(rows, "missing")).toBe("text");
  });
  it("finds a point in a point and in a polygon's first vertex", () => {
    expect(pointOf({ type: "Point", coordinates: [1, 2] })).toEqual([1, 2]);
    expect(pointOf({ type: "Polygon", coordinates: [[[3, 4], [5, 6]]] })).toEqual([3, 4]);
    expect(pointOf({ type: "Point", coordinates: ["x"] })).toBeNull();
    expect(pointOf("text")).toBeNull();
  });
  it("formats numbers, dates and points for people", () => {
    expect(format(1234)).toBe((1234).toLocaleString());
    expect(format(1.23456)).toBe((1.23456).toLocaleString(undefined, { maximumFractionDigits: 2 }));
    expect(format({ type: "Point", coordinates: [24.9, 60.1] })).toBe("60.10000, 24.90000");
    // A PostalAddress (T-3949) reads as one line; a part the source left out leaves no comma.
    const address = (parts: Record<string, string>) => format(parts as unknown as Cell);
    expect(address({ streetAddress: "Siltakatu 11", postalCode: "00580", addressLocality: "Helsinki", addressCountry: "FI" })).toBe(
      "Siltakatu 11, 00580 Helsinki",
    );
    expect(address({ streetAddress: "Rapparinkuja 8b", addressLocality: "Helsinki" })).toBe("Rapparinkuja 8b, Helsinki");
    expect(address({ addressLocality: "Espoo", addressCountry: "FI" })).toBe("Espoo");
    expect(address({ streetAddress: " ", addressCountry: "FI" })).toBe("");
    // Any other object without a type is empty text, never `undefined`.
    expect(format({ coordinates: null } as unknown as Cell)).toBe("");
    expect(format(null)).toBe("");
    expect(format(Number.NaN)).toBe("—");
    expect(format(Number.POSITIVE_INFINITY, "number")).toBe("—");
    expect(format(0)).toBe("0");
    expect(format("not a date", "date")).toBe("not a date");
  });
});

describe("filters and aggregates", () => {
  it("search, select and range each narrow; a blank filter passes all", () => {
    const filters = [
      { kind: "search" as const, attrs: ["name"] },
      { kind: "select" as const, attr: "name" },
      { kind: "range" as const, attr: "bikes" },
    ];
    expect(applyFilters(rows, filters, {}, "s")).toHaveLength(3);
    expect(applyFilters(rows, filters, { 0: "kal" }, "s").map((r) => r.id)).toEqual(["urn:b"]);
    expect(applyFilters(rows, filters, { 1: "Kamppi" }, "s").map((r) => r.id)).toEqual(["urn:a"]);
    // A range keeps rows without a number: a missing value is not "out of range".
    expect(applyFilters(rows, filters, { 2: [5, 20] }, "s").map((r) => r.id)).toEqual(["urn:a", "urn:c"]);
    // A filter on another source never touches these rows.
    expect(applyFilters(rows, [{ kind: "select", source: "other", attr: "name" }], { 0: "x" }, "s")).toHaveLength(3);
  });
  it("aggregates over the numbers present and answers null over none", () => {
    expect(aggregate(rows, "count")).toBe(3);
    expect(aggregate(rows, "sum", "bikes")).toBe(15);
    expect(aggregate(rows, "avg", "bikes")).toBe(7.5);
    expect(aggregate(rows, "min", "bikes")).toBe(3);
    expect(aggregate(rows, "max", "bikes")).toBe(12);
    expect(aggregate(rows, "sum", "name")).toBeNull();
    expect(aggregate([], "avg", "bikes")).toBeNull();
    expect(extent(rows, "bikes")).toEqual([3, 12]);
    expect(extent(rows, "name")).toBeNull();
  });
});

describe("entitiesUrl", () => {
  it("asks the run's endpoint for one type, its attributes and one page", () => {
    const url = entitiesUrl("s1ug", { name: "s", type: "Bike", attrs: ["a", "b"], q: "a>1" }, 0);
    expect(url.startsWith("/api/endpoint/s1ug/ngsi-ld/v1/entities?")).toBe(true);
    const params = new URLSearchParams(url.split("?")[1]);
    expect(params.get("type")).toBe("Bike");
    expect(params.get("options")).toBe("keyValues");
    expect(params.get("attrs")).toBe("a,b");
    expect(params.get("q")).toBe("a>1");
    expect(params.get("limit")).toBe("1000");
    expect(params.get("offset")).toBeNull();
  });
  it("never asks the endpoint for id or type as attributes", () => {
    const params = new URLSearchParams(entitiesUrl("s", { name: "s", type: "T", attrs: ["id", "a", "type"] }, 0).split("?")[1]);
    expect(params.get("attrs")).toBe("a");
    expect(new URLSearchParams(entitiesUrl("s", { name: "s", type: "T", attrs: ["id"] }, 0).split("?")[1]).get("attrs")).toBeNull();
  });
  it("pages with an offset and never asks past the source's limit", () => {
    const params = new URLSearchParams(entitiesUrl("s", { name: "s", type: "T", attrs: [], limit: 1200 }, 1000).split("?")[1]);
    expect(params.get("offset")).toBe("1000");
    expect(params.get("limit")).toBe("200");
    expect(new URLSearchParams(entitiesUrl("s", { name: "s", type: "T", attrs: [], limit: 99999 }, 0).split("?")[1]).get("limit")).toBe("1000");
  });

  it("names a row by its name, its title or the local id of its URN, never the whole URN", () => {
    const id = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001";
    expect(displayName({ id, type: "BikeHireDockingStation", name: "Kaivopuisto", title: "ignored" })).toBe("Kaivopuisto");
    expect(displayName({ id, type: "BikeHireDockingStation", name: "  ", title: "Station at the harbour" })).toBe("Station at the harbour");
    expect(displayName({ id, type: "BikeHireDockingStation", name: id })).toBe("001");
    expect(displayName({ id, type: "BikeHireDockingStation" })).toBe("001");
    expect(displayName({ id: "", type: "BikeHireDockingStation", name: null })).toBe("");
  });
});
