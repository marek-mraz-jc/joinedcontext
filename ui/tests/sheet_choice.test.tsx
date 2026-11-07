/**
 * JC1-07: the sheet choice of a workbook upload, alone: hidden for one sheet, every sheet by its
 * name with its size for several, the hint read as the select's description.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SheetChoice } from "../src/pages/spaces/SheetChoice";
import type { Sheet } from "../src/pages/spaces/importRows";

const sheet = (name: string, rows: number): Sheet => ({
  name,
  table: { columns: ["a", "b"], rows: Array.from({ length: rows }, () => ({ a: "1", b: "2" })) },
});

function choice(sheets: Sheet[], onChange = vi.fn()) {
  const { container } = render(
    <I18nextProvider i18n={i18n}>
      <SheetChoice id="sheet" sheets={sheets} value={sheets[0]?.name ?? ""} onChange={onChange} />
    </I18nextProvider>,
  );
  return { container, onChange };
}

describe("the sheet choice", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows nothing for a workbook of one sheet", () => {
    expect(choice([sheet("Only", 3)]).container).toBeEmptyDOMElement();
  });

  it("offers every sheet with its rows and columns, the hint as its description, and says the one chosen", async () => {
    const { onChange } = choice([sheet("Stations", 1), sheet("Stops", 12)]);
    const select = screen.getByRole("combobox", { name: "Sheet" });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Stations (1 row, 2 columns)", "Stops (12 rows, 2 columns)"]);
    expect(select).toHaveAccessibleDescription("One sheet is read at a time: choose the one with your rows, such as Stops.");
    await userEvent.setup().selectOptions(select, "Stops");
    expect(onChange).toHaveBeenCalledWith("Stops");
  });
});
