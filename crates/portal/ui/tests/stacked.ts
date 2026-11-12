/**
 * A table that turns into a card per row below 640 px (`Table`'s `stack`, jc-ui-tables): the
 * UI judge found tables of these pages cut off at the right edge at 400 px. Each row's cells
 * after the first carry the column's name, which the card prints beside the value.
 */
import { expect } from "vitest";

export function expectStacked(table: HTMLElement, labels: string[]): void {
  // The ui `Table` stacks by its `table-stack` class, the shared data grid by its frame's (T-6739).
  expect(table.classList.contains("table-stack") || table.closest(".jc-data-grid--stack") !== null).toBe(true);
  const rows = [...table.querySelectorAll("tbody > tr")];
  expect(rows.length).toBeGreaterThan(0);
  for (const row of rows) {
    const named = [...row.querySelectorAll("td[data-label]")].map((cell) => cell.getAttribute("data-label"));
    expect(named).toEqual(labels);
  }
}
