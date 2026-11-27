/**
 * T-7937: an actions column's `text` is `noText`, and the grid never reads it: search and the
 * filters skip an actions column, and a sort names only a column whose header offers one.
 */
import { describe, expect, it, vi } from "vitest";
import { arrangeRows, EMPTY_QUERY } from "@joinedcontext/sdk/grid";
import type { DataColumn } from "@joinedcontext/sdk/grid";
import { noText } from "../src/components/grid/noText";

interface Row {
  name: string;
}

const ROWS: Row[] = [{ name: "air-quality" }, { name: "parking" }, { name: "street-works" }];

describe("an actions column's text", () => {
  it("is empty", () => {
    expect(noText()).toBe("");
  });

  it("is never read by a search, a filter or a page of the rows", () => {
    const text = vi.fn(noText);
    const columns: DataColumn<Row>[] = [
      { key: "name", label: "Name", text: (row) => row.name },
      { key: "actions", label: "Actions", actions: true, text, cell: () => null },
    ];
    const searched = arrangeRows(ROWS, columns, { ...EMPTY_QUERY, search: "park" });
    expect(searched.rows.map((row) => row.name)).toEqual(["parking"]);
    const filtered = arrangeRows(ROWS, columns, {
      ...EMPTY_QUERY,
      filters: { name: { op: "contains", value: "street" }, actions: { op: "contains", value: "x" } },
    });
    expect(filtered.rows.map((row) => row.name)).toEqual(["street-works"]);
    expect(arrangeRows(ROWS, columns, { ...EMPTY_QUERY, search: "" }).total).toBe(3);
    expect(text).not.toHaveBeenCalled();
  });
});
