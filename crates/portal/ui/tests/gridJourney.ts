/**
 * One walk through a list on the shared grid (T-6751): the page's real rows, sorted from the
 * column menu, searched, a column hidden from Arrange columns, and the phone's card layout on.
 * Each moved ops page runs it on its own fixtures, so a page that fell back to a hand table fails.
 */
import { fireEvent, screen, within } from "@testing-library/react";
import { expect } from "vitest";
import en from "../src/locales/en.json";

export interface GridWalk {
  /** The table's caption. */
  caption: string;
  /** A column label whose menu sorts ascending to `sorted`. */
  sortBy: string;
  /** The row names (the first column) after that sort. */
  sorted: string[];
  /** A word typed in the grid's search, and the row names left. */
  search: string;
  matches: string[];
  /** A column label Arrange columns hides. */
  hide: string;
  /** What names a row; its first cell when absent. */
  name?: (row: HTMLElement) => string;
}

export async function walkSharedGrid(walk: GridWalk): Promise<void> {
  const table = await screen.findByRole("table", { name: walk.caption });
  const grid = table.closest(".jc-data-grid") as HTMLElement;
  expect(grid).not.toBeNull();
  expect(grid).toHaveClass("jc-data-grid--stack");
  const scope = within(grid);
  const name = walk.name ?? ((row: HTMLElement) => row.querySelector("th")?.textContent ?? "");
  const names = () =>
    [...table.querySelectorAll<HTMLElement>("tbody tr")]
      .filter((row) => !row.querySelector(".jc-grid-empty"))
      .map((row) => name(row).trim());

  fireEvent.click(scope.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${walk.sortBy}` }));
  fireEvent.click(scope.getAllByRole("menuitemradio")[0]);
  expect(names()).toEqual(walk.sorted);

  const search = scope.getByRole("searchbox", { name: en.dataGrid.searchRows });
  fireEvent.change(search, { target: { value: walk.search } });
  expect(names()).toEqual(walk.matches);
  fireEvent.change(search, { target: { value: "no-row-says-this-zz" } });
  expect(scope.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
  fireEvent.click(scope.getByRole("button", { name: en.entityGrid.clearFilters }));
  expect(names()).toHaveLength(walk.sorted.length);

  fireEvent.click(scope.getByRole("button", { name: en.entityGrid.arrangeColumns }));
  const panel = scope.getByRole("dialog", { name: en.entityGrid.arrangeColumns });
  fireEvent.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${walk.hide}` }));
  expect(within(table).queryByRole("columnheader", { name: new RegExp(walk.hide) })).toBeNull();
  // Shown again: the layout is kept per browser, and the next test reads the page as it opens.
  fireEvent.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${walk.hide}` }));
}
