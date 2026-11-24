// T-7706: the unit picker keeps the chosen unit among the options even when the search no longer finds it.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { UnitPicker } from "../src/components/pickers/UnitPicker";

describe("UnitPicker, a search that misses the chosen unit", () => {
  it("still lists the chosen unit beside what the search found", async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <UnitPicker label="Unit" value="GQ" onChange={() => undefined} />
      </I18nextProvider>,
    );
    const box = screen.getByRole("combobox", { name: "Unit" });
    await userEvent.clear(box);
    await userEvent.type(box, "millisecond");
    const list = screen.getByRole("listbox", { name: "Unit" });
    expect(within(list).getByText("ms — millisecond")).toBeInTheDocument();
    expect(within(list).getByText("µg/m³ — microgram per cubic metre")).toBeInTheDocument();
  });
});
