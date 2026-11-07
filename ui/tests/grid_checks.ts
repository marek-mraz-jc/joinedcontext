/**
 * T-6205: the shared grid's search, sort and state filter, driven as a person does on a list
 * converted to ManifestGrid. Each converted list's test calls `narrowsAndOrders` once.
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect } from "vitest";
import en from "../src/locales/en.json";

/** The name of each body row: the first element of the grid's name cell (a badge beside it is left out). */
export function names(table: HTMLElement): string[] {
  return within(table)
    .getAllByRole("row")
    .slice(1)
    .map((row) => {
      const cell = within(row).getAllByRole("cell")[0];
      return (cell?.firstElementChild ?? cell)?.textContent ?? "";
    });
}

export async function narrowsAndOrders(
  table: HTMLElement,
  check: {
    /** The grid's search label (`grid.searchIn`), a text that keeps `searched` alone. */
    search: { label: string; text: string; kept: string[] };
    /** A numeric column's label and the names in ascending order of it. */
    sort: { column: string; ascending: string[] };
    /** A state value's label and the names it keeps. */
    filter: { value: RegExp; kept: string[] };
  },
): Promise<void> {
  const grid = table.closest<HTMLElement>("[data-manifest-grid]");
  if (!grid) throw new Error("the table is no shared grid");
  const user = userEvent.setup();
  const box = within(grid).getByRole("searchbox", { name: check.search.label });
  await user.type(box, check.search.text);
  expect(names(table)).toEqual(check.search.kept);
  await user.clear(box);

  const sort = within(table).getByRole("button", { name: en.grid.sortBy.replace("{column}", check.sort.column) });
  await user.click(sort);
  expect(names(table)).toEqual(check.sort.ascending);
  await user.click(sort);
  expect(names(table)).toEqual([...check.sort.ascending].reverse());
  await user.click(sort);

  await user.click(within(grid).getByText(en.grid.filter));
  await user.click(screen.getByRole("checkbox", { name: check.filter.value }));
  expect(names(table)).toEqual(check.filter.kept);
}
