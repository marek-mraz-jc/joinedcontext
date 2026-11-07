/**
 * T-6305 (G-R2GRID2-001): `@joinedcontext/sdk/grid` renders over any store. The adapter below reads
 * plain records, nothing NGSI-LD: it builds the rich rows itself, answers the caller's decisions as
 * `access`, and the visible strings come in as `labels`, as a Portal page or an App injects them.
 */
import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { EntityGrid, parseGridConfig } from "../src/sdk/grid";
import type { AttributeAccess, EntitySource, RichRow } from "../src/sdk/grid";

/** A library's own rows: a key, a title, a count. */
const BOOKS = [
  { isbn: "978-80-1", title: "Krvavé sonety", copies: 3 },
  { isbn: "978-80-2", title: "Rozprávky", copies: 0 },
];

function bookRow(book: (typeof BOOKS)[number]): RichRow {
  return {
    id: book.isbn,
    type: "Book",
    cells: { title: { kind: "property", value: book.title }, copies: { kind: "property", value: book.copies } },
    raw: book,
  };
}

function recordSource(asked: string[]): EntitySource {
  return {
    async query(q, page) {
      asked.push(q.type);
      const rows = BOOKS.map(bookRow);
      return { rows: rows.slice(page.offset, page.offset + page.limit), total: rows.length };
    },
    async get(id) {
      const book = BOOKS.find((b) => b.isbn === id);
      return book ? bookRow(book) : null;
    },
  };
}

const ACCESS: AttributeAccess[] = [
  { name: "title", read: true, write: false, delete: false, reason: "grant" },
  { name: "copies", read: true, write: false, delete: false, reason: "grant" },
];

describe("the grid entry over a non-NGSI-LD source", () => {
  it("renders the adapter's rows with the injected labels", async () => {
    const parsed = parseGridConfig({
      source: { kind: "fixture", name: "books" },
      type: "Book",
      columns: [{ attr: "title", label: "Názov" }, { attr: "copies", label: "Výtlačky" }],
    });
    expect(parsed.findings).toEqual([]);
    const asked: string[] = [];
    render(<EntityGrid config={parsed.config!} source={recordSource(asked)} access={ACCESS} labels={{ id: "Kľúč", next: "Ďalej" }} />);
    const grid = screen.getByRole("grid");
    expect(await within(grid).findByText("Krvavé sonety")).toBeInTheDocument();
    expect(within(grid).getByText("Rozprávky")).toBeInTheDocument();
    expect(within(grid).getByRole("columnheader", { name: /Názov/ })).toBeInTheDocument();
    expect(within(grid).getByRole("columnheader", { name: /Kľúč/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Ďalej" })).toBeInTheDocument();
    expect(asked).toContain("Book");
  });
});
