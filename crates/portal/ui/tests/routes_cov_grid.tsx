/**
 * The grid helpers the route coverage tests share: every column of an admin grid sorted from its
 * header menu, so a column's `text` is what the person's sort reads, not an unexercised line.
 */
import { screen, waitFor, within } from "@testing-library/react";
import { expect } from "vitest";
import type { UserEvent } from "@testing-library/user-event";
import en from "../src/locales/en.json";

const ASCENDING = new RegExp(
  `^\\W*(${en.entityGrid.sortAsc}|${en.entityGrid.sortAscDate}|${en.entityGrid.sortAscNumber}|${en.entityGrid.sortAscBoolean})$`,
);

/** The `<tbody>` rows of the grid, as their first cell's text, in the order shown. */
export function firstCells(table: HTMLElement): string[] {
  return within(table)
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.querySelector("td, th[scope='row']")?.textContent ?? "");
}

/**
 * Sort the grid by each column in turn (ascending, from the header's Column options menu) and
 * return the first cells after each sort, keyed by the column's label.
 */
export async function sortEachColumn(
  user: UserEvent,
  table: HTMLElement,
): Promise<Record<string, string[]>> {
  const seen: Record<string, string[]> = {};
  const buttons = within(table).getAllByRole("button", {
    name: new RegExp(`^${en.entityGrid.columnMenu}: `),
  });
  for (const button of buttons) {
    const label = (button.getAttribute("aria-label") ?? "").slice(
      `${en.entityGrid.columnMenu}: `.length,
    );
    await user.click(button);
    const asc = (await screen.findAllByRole("menuitemradio")).find((item) =>
      ASCENDING.test(item.textContent?.trim() ?? ""),
    );
    if (!asc) throw new Error(`no ascending sort for ${label}`);
    await user.click(asc);
    seen[label] = firstCells(table);
  }
  return seen;
}

/**
 * Press Check when Propose is still closed behind "run the check first", wait for the green
 * verdict to open it, then press Propose of the dialog (`proposeName`).
 */
export async function proposeWhenGreen(
  user: UserEvent,
  dialog: HTMLElement,
  proposeName: string,
): Promise<void> {
  const propose = within(dialog).getByRole("button", { name: proposeName });
  if (propose.getAttribute("aria-disabled") === "true") {
    await user.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
  }
  await user.click(propose);
}
