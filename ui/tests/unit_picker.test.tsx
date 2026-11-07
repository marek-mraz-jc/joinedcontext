/**
 * The unit picker (T-2809, DM-06): the whole of Recommendation 20, searched by code, symbol,
 * UCUM, name or quantity kind, the frequent units first, operated by keyboard.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { UnitPicker } from "../src/components/pickers/UnitPicker";

function Harness({ initial = "", clearable = false, onChange }: { initial?: string; clearable?: boolean; onChange: (code: string) => void }) {
  const [value, setValue] = useState(initial);
  return (
    <I18nextProvider i18n={i18n}>
      <UnitPicker
        label="Unit"
        value={value}
        clearable={clearable}
        onChange={(code) => {
          setValue(code);
          onChange(code);
        }}
      />
    </I18nextProvider>
  );
}

describe("UnitPicker", () => {
  it("opens on the frequent units and names the chosen one by symbol and name", async () => {
    render(<Harness initial="GQ" onChange={vi.fn()} />);
    const box = screen.getByRole("combobox", { name: "Unit" });
    expect(box).toHaveValue("µg/m³ — microgram per cubic metre");
    await userEvent.click(box);
    const list = screen.getByRole("listbox", { name: "Unit" });
    const options = within(list).getAllByRole("option");
    expect(options.length).toBeGreaterThan(20);
    expect(within(list).getByText("°C — degree Celsius")).toBeInTheDocument();
    expect(within(list).queryByText("hectolitre of pure alcohol")).not.toBeInTheDocument();
  });

  it("finds µg/m³ when a person types ug/m3 and picks it by keyboard", async () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const box = screen.getByRole("combobox", { name: "Unit" });
    await userEvent.type(box, "ug/m3");
    const first = within(screen.getByRole("listbox", { name: "Unit" })).getAllByRole("option")[0];
    expect(first).toHaveTextContent("µg/m³ — microgram per cubic metre");
    await userEvent.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith("GQ");
    expect(box).toHaveValue("µg/m³ — microgram per cubic metre");
  });

  it("finds a rare unit by name under its quantity kind, and says when the list is cut", async () => {
    render(<Harness onChange={vi.fn()} />);
    const box = screen.getByRole("combobox", { name: "Unit" });
    await userEvent.type(box, "millisecond");
    expect(within(screen.getByRole("listbox", { name: "Unit" })).getByText("ms — millisecond")).toBeInTheDocument();
    await userEvent.clear(box);
    await userEvent.type(box, "e");
    expect(screen.getByText(/Showing 100 of \d+ units/)).toBeInTheDocument();
  });

  it("says how to search when nothing matches", async () => {
    render(<Harness onChange={vi.fn()} />);
    await userEvent.type(screen.getByRole("combobox", { name: "Unit" }), "zzqx");
    expect(screen.getByText(en.units.noMatch)).toBeInTheDocument();
  });

  it("offers no unit first where the field may have none", async () => {
    const onChange = vi.fn();
    render(<Harness initial="CEL" clearable onChange={onChange} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Unit" }));
    const first = within(screen.getByRole("listbox", { name: "Unit" })).getAllByRole("option")[0];
    expect(first).toHaveTextContent(en.units.none);
    await userEvent.click(first);
    expect(onChange).toHaveBeenCalledWith("");
  });
});
