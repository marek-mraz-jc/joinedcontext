/**
 * T-2811 (DM-06): a slot's unit in the pipeline workbench, and the line that converts a source
 * in another unit with the code list's factor, never a typed one.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SlotUnit, conversionLine } from "../src/pages/pipelines/SlotUnit";
import { unitOf } from "../src/units";

const unit = (code: string) => unitOf(code)!;

describe("conversionLine", () => {
  it("scales mg/m³ into µg/m³ and writes the model's code", () => {
    expect(conversionLine("pm10", unit("GP"), unit("GQ"))).toBe(
      'root.pm10 = { "type": "Property", "value": this.pm10.number() * 1000, "unitCode": "GQ" }',
    );
  });

  it("carries the offset of a temperature, and reads the value unchanged in the same unit", () => {
    expect(conversionLine("t", unit("FAH"), unit("CEL"))).toBe(
      'root.t = { "type": "Property", "value": this.t.number() * 0.555555555556 - 17.7777777778, "unitCode": "CEL" }',
    );
    expect(conversionLine("t", unit("CEL"), unit("CEL"))).toBe(
      'root.t = { "type": "Property", "value": this.t.number(), "unitCode": "CEL" }',
    );
  });

  it("offers no line for units of different quantities", () => {
    expect(conversionLine("pm10", unit("CEL"), unit("GQ"))).toBeUndefined();
    expect(conversionLine("level", unit("2N"), unit("P1"))).toBeUndefined();
  });
});

describe("SlotUnit", () => {
  it("names the model's unit and adds the conversion of a source picked among convertible units", async () => {
    const onAdd = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <SlotUnit slot="pm10" code="GQ" onAdd={onAdd} />
      </I18nextProvider>,
    );
    expect(screen.getByText(en.pipelines.workbench.mapping.measuredIn.replace("{symbol}", "µg/m³").replace("{code}", "GQ"))).toBeInTheDocument();
    const box = screen.getByRole("combobox", { name: en.pipelines.workbench.mapping.sourceUnit });
    await userEvent.type(box, "celsius");
    expect(screen.getByText(/No UN\/CEFACT unit matches/)).toBeInTheDocument();
    await userEvent.clear(box);
    await userEvent.type(box, "mg/m3");
    const list = screen.getByRole("listbox");
    expect(within(list).queryByText(/µg\/m³ — microgram per cubic metre/)).not.toBeInTheDocument();
    await userEvent.click(within(list).getByText("mg/m³ — milligram per cubic metre"));
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.workbench.mapping.addConversion }));
    expect(onAdd).toHaveBeenCalledWith('root.pm10 = { "type": "Property", "value": this.pm10.number() * 1000, "unitCode": "GQ" }');
  });

  it("draws nothing for a code the list does not know", () => {
    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <SlotUnit slot="pm10" code="XQZ" onAdd={vi.fn()} />
      </I18nextProvider>,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
