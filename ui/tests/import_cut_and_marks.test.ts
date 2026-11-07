// covers: src/pages/spaces/importRows.ts
/** F-UI3-011..013: a cut file says so, a file's own decimal mark decides, and a sample reads in the file's encoding. */
import { describe, expect, it } from "vitest";
import { decimalMarkOf, decodeAs, MAX_ROWS, numberOf, parseCsv, parseJson, toEntities } from "../src/pages/spaces/importRows";

describe("a file longer than MAX_ROWS", () => {
  it("keeps MAX_ROWS rows and the file's own count", () => {
    const csv = ["name", ...Array.from({ length: MAX_ROWS + 1 }, (_, n) => `row${n}`)].join("\n");
    const table = parseCsv(csv);
    expect(table.rows).toHaveLength(MAX_ROWS);
    expect(table.total).toBe(MAX_ROWS + 1);
    expect(parseJson(JSON.stringify(Array.from({ length: MAX_ROWS + 2 }, (_, n) => ({ n })))).total).toBe(MAX_ROWS + 2);
    expect(parseCsv("name\na\nb").total).toBeUndefined();
  });
});

describe("the decimal mark of a file", () => {
  it("reads 1,234 as a thousand and more in a file that writes 4.5", () => {
    const table = parseCsv("id,capacity\na,\"1,234\"\nb,4.5\n");
    expect(decimalMarkOf(table)).toBe(".");
    expect(numberOf("1,234", ".")).toBe(1234);
    const { entities } = toEntities(
      table,
      { id: "id", capacity: "capacity" },
      [{ name: "capacity", range: "float", kind: "Property", required: false }],
      { type: "Parking", orgDomain: "hel.fi", space: "parking" },
    );
    expect(entities.map((one) => (one.entity.capacity as { value: number }).value)).toEqual([1234, 4.5]);
  });

  it("keeps the comma a decimal mark in a file that writes 4,5, and guesses as before when nothing tells", () => {
    expect(decimalMarkOf(parseCsv("id;capacity\na;4,5\nb;1.234,5\n"))).toBe(",");
    expect(numberOf("1.234,5", ",")).toBe(1234.5);
    expect(decimalMarkOf(parseCsv("id,capacity\na,\"1,234\"\n"))).toBeUndefined();
    expect(numberOf("4,5")).toBe(4.5);
  });
});

describe("a sample of a Windows-1250 file", () => {
  it("reads as the file did", () => {
    const bytes = new Uint8Array([0x9a, 0x74, 0x72, 0x65, 0x65, 0x74, 0x3b, 0xe8, 0xed, 0x73, 0x6c, 0x6f]); // "štreet;číslo"
    expect(decodeAs(bytes, "windows-1250")).toBe("štreet;číslo");
    expect(decodeAs(bytes, undefined)).toBe("štreet;číslo");
  });
});
