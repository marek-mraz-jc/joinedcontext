import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import type { EntitySource, RichRow } from "@joinedcontext/sdk";
import { DATASETS, exportCsv, gridConfig, plain, SPEC } from "./datasets";
import { AIR, MONUMENTS, STATIONS, WORKS } from "./fixtures/data";

const rows = (entities: unknown[]): RichRow[] => entities.map((entity) => toRichRow(entity as Record<string, unknown>));

/** A source over a list, paged the way the endpoint pages. */
function over(list: RichRow[], seen: number[] = []): EntitySource {
  return {
    async query(_q, page) {
      seen.push(page.offset);
      return { rows: list.slice(page.offset, page.offset + page.limit), total: list.length } as never;
    },
    async get() {
      return null;
    },
  };
}

describe("the datasets", () => {
  it("are read only and filter only on what the endpoint can answer", () => {
    for (const dataset of DATASETS) {
      const grid = gridConfig(dataset, "slug", {});
      expect(grid.mode).toBe("view");
      expect(grid.editableAttrs).toEqual([]);
      expect(grid.columns.map((c) => c.attr)).toEqual([...SPEC[dataset].columns]);
      for (const attr of grid.filters.allowed ?? []) expect(SPEC[dataset].columns).toContain(attr);
    }
  });

  it("show a reading with its hour, and a link as a link", () => {
    const air = gridConfig("air", "slug", {}).columns;
    expect(air.find((c) => c.attr === "co")?.show).toEqual({ observedAt: true });
    expect(gridConfig("works", "slug", {}).columns.find((c) => c.attr === "url")?.format).toBe("link");
  });
});

describe("plain", () => {
  it("writes a name in the reader's language, a number as a number and a missing value as nothing", () => {
    // A title in one language is written in it whatever the reader's language is.
    const [work] = rows(WORKS);
    expect(plain(work.cells.name, "en")).toBe(Object.values(WORKS[0].name.languageMap)[0]);
    const [station] = rows(AIR);
    expect(plain(station.cells.name, "en")).toBe("Station SK0020A, urban background");
    expect(plain(station.cells.co, "sk")).toBe(0.63452);
    expect(plain(station.cells.missing, "sk")).toBeNull();
    const zilina = rows(STATIONS).find((row) => plain(row.cells.name, "sk") === "Žilina") as RichRow;
    expect(plain(zilina.cells.dailyDepartures, "sk")).toBe(166);
  });
});

describe("plain, of what a source writes otherwise", () => {
  const cell = (attrs: Record<string, unknown>) => toRichRow({ id: "urn:ngsi-ld:Thing:x", type: "Thing", a: attrs }).cells.a;

  it("writes a name in Slovak, else any language, and nothing for an empty map", () => {
    const name = (languageMap: Record<string, string>) => cell({ type: "LanguageProperty", languageMap });
    expect(plain(name({ sk: "Hrad", de: "Burg" }), "en")).toBe("Hrad");
    expect(plain(name({ de: "Burg" }), "en")).toBe("Burg");
    expect(plain(name({}), "en")).toBeNull();
  });

  it("writes a relationship's object, several as one field, and none as nothing", () => {
    expect(plain(cell({ type: "Relationship", object: "urn:a" }), "sk")).toBe("urn:a");
    expect(plain(cell({ type: "Relationship", object: ["urn:a", "urn:b"] }), "sk")).toBe("urn:a urn:b");
    expect(plain(cell({ type: "Relationship" }), "sk")).toBeNull();
  });

  it("writes a value as it is, a typed literal by its value, a window by its ends, a point whole, anything else as JSON", () => {
    expect(plain(cell({ type: "Property", value: null }), "sk")).toBeNull();
    expect(plain(cell({ type: "Property", value: true }), "sk")).toBe(true);
    expect(plain(cell({ type: "Property", value: "x" }), "sk")).toBe("x");
    expect(plain(cell({ type: "Property", value: { "@type": "DateTime", "@value": "2026-10-06T19:00:00Z" } }), "sk")).toBe("2026-10-06T19:00:00Z");
    expect(plain(cell({ type: "Property", value: { start: "2025-01-01", end: "2025-12-31" } }), "sk")).toBe("2025-01-01/2025-12-31");
    expect(plain(cell({ type: "GeoProperty", value: { type: "Point", coordinates: [18.7, 49.2] } }), "sk")).toEqual({ type: "Point", coordinates: [18.7, 49.2] });
    expect(plain(cell({ type: "Property", value: { a: 1 } }), "sk")).toBe('{"a":1}');
    expect(plain(cell({ type: "Property", value: [1, 2] }), "sk")).toBe("[1,2]");
    expect(plain([cell({ type: "Property", value: 1 }), cell({ type: "Property", value: 2 })].flat() as never, "sk")).toBe(1);
  });

  it("reads a cell a source hands over as it is, without the SDK's row", () => {
    const raw = (c: Record<string, unknown>) => plain(c as never, "en");
    expect(raw({ kind: "property", languageMap: {} })).toBeNull();
    expect(raw({ kind: "property", languageMap: { de: "Burg" } })).toBe("Burg");
    expect(raw({ kind: "relationship" })).toBeNull();
    expect(raw({ kind: "property" })).toBeNull();
    expect(raw({ kind: "property", value: { "@value": "2026-10-06" } })).toBe("2026-10-06");
    expect(raw({ kind: "property", value: { start: "2025-01-01", end: "2025-12-31" } })).toBe("2025-01-01/2025-12-31");
    expect(raw({ kind: "property", value: { type: "Point", coordinates: [18.7, 49.2] } })).toEqual({ type: "Point", coordinates: [18.7, 49.2] });
    expect(plain(undefined, "en")).toBeNull();
  });
});

describe("exportCsv", () => {
  it("reads every page and writes one row per entity under the attribute names", async () => {
    const many = Array.from({ length: 1201 }, (_, index) => rows(MONUMENTS)[index % MONUMENTS.length]);
    const seen: number[] = [];
    const result = await exportCsv("monuments", over(many, seen), "sk");
    expect(seen).toEqual([0, 500, 1000]);
    expect(result).toMatchObject({ rows: 1201, truncated: false });
    const text = await result.file.text();
    const lines = text.replace(/^﻿/, "").split("\r\n");
    expect(lines[0]).toBe(["id", ...SPEC.monuments.columns].join(","));
    expect(lines).toHaveLength(1202);
    // A column with no address is an empty field, never "null".
    expect(text).not.toContain("null");
  });

  it("says when it stopped at its ceiling", async () => {
    const many = Array.from({ length: 5000 }, () => rows(WORKS)[0]);
    expect(await exportCsv("works", over(many), "sk")).toMatchObject({ rows: 5000, truncated: true });
  });
});
