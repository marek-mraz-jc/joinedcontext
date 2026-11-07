/**
 * The narrowing this application is, without a table or a network (T-2436, T-2437).
 *
 * Two properties are worth more than the rest: what the screen sends is the note and nothing
 * else, and a note the model would refuse never leaves the browser.
 */
import { describe, expect, it, vi } from "vitest";
import { SourceError } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { COLUMNS, gridConfig, NOTE, NOTE_MAX, noteFault, notesOnly } from "./records";
import { LOCALES, noteWords } from "./locales";

const words = noteWords(LOCALES.en);
const SLUG = "ovr4ttzywhad2oiogf67n7zyn2g2elfc";
const ID = "urn:ngsi-ld:KeyPerformanceIndicator:banskabystrica.sk:banskabystrica-mesto:vh5003rr-SK0321508438-2023-U03084";

function stub(): EntitySource & { patched: Array<[string, Record<string, unknown>]> } {
  const patched: Array<[string, Record<string, unknown>]> = [];
  return {
    patched,
    query: vi.fn(async () => ({ rows: [], total: 0 })),
    get: vi.fn(async () => null),
    history: vi.fn(async () => []),
    patch: vi.fn(async (id: string, attrs: Record<string, unknown>) => {
      patched.push([id, attrs]);
    }),
    remove: vi.fn(async () => undefined),
  };
}

describe("what a note may hold", () => {
  it("takes a note a person would actually write", () => {
    expect(noteFault({ [NOTE]: { type: "Property", value: "Overené proti ročenke." } }, words)).toBeNull();
  });

  it("takes an emptied note, which is how a note is removed", () => {
    expect(noteFault({ [NOTE]: { type: "Property", value: "" } }, words)).toBeNull();
    expect(noteFault({ [NOTE]: { type: "Property", value: null } }, words)).toBeNull();
  });

  it("refuses a note past the bound the model declares, and says the bound", () => {
    const fault = noteFault({ [NOTE]: { type: "Property", value: "a".repeat(NOTE_MAX + 1) } }, words);
    expect(fault).toBe(words.tooLong(NOTE_MAX));
    expect(noteFault({ [NOTE]: { type: "Property", value: "a".repeat(NOTE_MAX) } }, words)).toBeNull();
  });

  it("refuses the characters the model's pattern refuses", () => {
    expect(noteFault({ [NOTE]: { type: "Property", value: "<script>alert(1)</script>" } }, words)).toBe(
      words.forbidden,
    );
  });

  it("refuses any attribute that is not the note, whatever it holds", () => {
    expect(noteFault({ value: { type: "Property", value: 1 } }, words)).toBe(words.notText);
    expect(noteFault({ [NOTE]: { type: "Property", value: 7 } }, words)).toBe(words.notText);
  });
});

describe("the surface the screen holds", () => {
  it("sends a good note to the endpoint, unchanged", async () => {
    const base = stub();
    const source = notesOnly(base, words);
    await source.patch!(ID, { [NOTE]: { type: "Property", value: "Sedí." } });
    expect(base.patched).toEqual([[ID, { [NOTE]: { type: "Property", value: "Sedí." } }]]);
  });

  it("never sends a note the model would refuse, and says why in the reader's language", async () => {
    const base = stub();
    const source = notesOnly(base, words);
    await expect(
      source.patch!(ID, { [NOTE]: { type: "Property", value: "<b>" } }),
    ).rejects.toThrow(words.forbidden);
    expect(base.patched).toEqual([]);
    await expect(source.patch!(ID, { value: { type: "Property", value: 1 } })).rejects.toBeInstanceOf(
      SourceError,
    );
    expect(base.patched).toEqual([]);
  });

  it("cannot delete a published row at all", () => {
    // The grid offers no deletion today; the surface it is handed could not perform one anyway.
    expect(notesOnly(stub(), words).remove).toBeUndefined();
  });

  it("is a reader when the endpoint takes no writes", () => {
    const readOnly = { query: vi.fn(), get: vi.fn() } as unknown as EntitySource;
    expect(notesOnly(readOnly, words).patch).toBeUndefined();
  });
});

describe("the grid this application is", () => {
  const config = gridConfig(SLUG, LOCALES.sk.column, true);

  it("reads one type through its own endpoint, by slug and never by URL", () => {
    expect(config.source).toEqual({ kind: "endpoint", slug: SLUG });
    expect(config.type).toBe("KeyPerformanceIndicator");
  });

  it("opens the note and nothing else", () => {
    expect(config.mode).toBe("edit");
    expect(config.editableAttrs).toEqual([NOTE]);
    const editable = config.columns.filter((column) => column.editable);
    expect(editable.map((column) => column.attr)).toEqual([NOTE]);
  });

  it("opens nothing for a reader whose grant has no updateAttrs on the note (T-5137)", () => {
    const reader = gridConfig(SLUG, LOCALES.sk.column, false);
    expect(reader.mode).toBe("view");
    expect(reader.editableAttrs).toEqual([]);
    expect(reader.columns.filter((column) => column.editable)).toEqual([]);
    expect(reader.columns.map((column) => column.attr)).toEqual([...COLUMNS]);
  });

  it("shows the columns a person recognises a record by, with the unit on the number", () => {
    expect(config.columns.map((column) => column.attr)).toEqual([...COLUMNS]);
    // The unit is drawn beside the number from the publisher's text (cells.tsx), not as a
    // column of UN/CEFACT codes (T-2966).
    expect(config.columns.find((column) => column.attr === "kpiValue")?.show).toBeUndefined();
    for (const column of config.columns) {
      expect(column.label, column.attr).toBe(LOCALES.sk.column[column.attr]);
    }
  });

  it("filters on the columns it shows, so the endpoint answers the filter and not the browser", () => {
    expect(config.filters.allowed).toEqual([...COLUMNS]);
    expect(config.pageSize).toBeLessThanOrEqual(25);
  });

  it("offers no history, because the grant reaches no temporal operation", () => {
    expect(config.history.enabled).toBe(false);
    expect(config.rowActions).toEqual([]);
    expect(config.map.enabled).toBe(false);
  });
});
