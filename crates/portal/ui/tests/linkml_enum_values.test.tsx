/**
 * T-2706, UI-86: the editor writes what the grid shows for an enum value (its title and its
 * description), removes a value, and gives each enum its own box for a new value.
 */
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { enumUses } from "../src/pages/models/EnumValues";
import { LinkmlVisualEditor } from "../src/pages/models/LinkmlVisualEditor";
import { NameField } from "../src/pages/models/NameField";
import { parseModel } from "../src/pages/models/linkml";
import { applyOperations, type Operation } from "../src/pages/models/operations";

const SOURCE = `# alerts
id: https://hel.fi/models/alerts
name: alerts
classes: {}
slots: {}
enums:
  AlertCategory:
    permissible_values:
      traffic:
        title: Traffic
      weather: {}
      health:
  Severity:
    permissible_values:
      low: {}
`;

const values = (source: string, name = "AlertCategory") =>
  parseModel(source).enums.find((entry) => entry.name === name)?.permissible_values ?? [];

describe("enum value operations", () => {
  it("writes a title per language, keeping a plain-string title as the English one", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "setEnumValue", enum: "AlertCategory", value: "traffic", field: "title", locale: "sk", text: "Doprava" },
      { op: "setEnumValue", enum: "AlertCategory", value: "health", field: "title", locale: "en", text: "Health" },
      { op: "setEnumValue", enum: "AlertCategory", value: "weather", field: "description", text: "Storms, ice" },
    ]);
    expect(refused).toEqual([]);
    expect(values(source)).toEqual([
      { name: "traffic", title: { en: "Traffic", sk: "Doprava" } },
      { name: "weather", description: "Storms, ice" },
      { name: "health", title: { en: "Health" } },
    ]);
    // The comment and the other enum survive the edit.
    expect(source.startsWith("# alerts\n")).toBe(true);
    expect(values(source, "Severity").map((value) => value.name)).toEqual(["low"]);
  });

  it("removes a title or a description when its text is emptied", () => {
    const { source } = applyOperations(SOURCE, [
      { op: "setEnumValue", enum: "AlertCategory", value: "traffic", field: "title", locale: "en", text: "" },
    ]);
    expect(values(source)[0]).toEqual({ name: "traffic" });
    expect(source).not.toContain("title");
  });

  it("removes a value and leaves the others", () => {
    const { source, refused } = applyOperations(SOURCE, [{ op: "removeEnumValue", enum: "AlertCategory", value: "weather" }]);
    expect(refused).toEqual([]);
    expect(values(source).map((value) => value.name)).toEqual(["traffic", "health"]);
  });

  it("refuses a value or an enum the model does not have, and a title without a language", () => {
    const refusedOf = (operation: Parameters<typeof applyOperations>[1][number]) =>
      applyOperations(SOURCE, [operation]).refused.map((one) => one.reason);
    expect(refusedOf({ op: "removeEnumValue", enum: "AlertCategory", value: "fire" })).toEqual([
      "enum 'AlertCategory' has no value 'fire'",
    ]);
    expect(refusedOf({ op: "setEnumValue", enum: "Nope", value: "x", field: "description", text: "y" })).toEqual([
      "unknown enum 'Nope'",
    ]);
    expect(refusedOf({ op: "setEnumValue", enum: "AlertCategory", value: "traffic", field: "title", text: "T" })).toEqual([
      "a title needs the language it is written in",
    ]);
  });
});

function Harness() {
  const [source, setSource] = useState(SOURCE);
  return (
    <>
      <LinkmlVisualEditor source={source} onChange={setSource} locales={["sk", "en"]} />
      <textarea readOnly aria-label="source" value={source} />
    </>
  );
}

describe("the editor's enums", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    render(
      <I18nextProvider i18n={i18n}>
        <Harness />
      </I18nextProvider>,
    );
  });

  const source = () => (screen.getByLabelText("source") as HTMLTextAreaElement).value;
  const openEnum = (name: string) => userEvent.click(screen.getByRole("button", { name: new RegExp(`^${name}`) }));

  /// T-6162: an enumeration opens its values table from the rail; a stored value is no input.
  it("opens an enumeration's values from the rail and adds one, each enum with its own box", async () => {
    await openEnum("AlertCategory");
    const table = screen.getByRole("table", { name: en.models.enumValues.caption.replace("{name}", "AlertCategory") });
    expect(table).toHaveTextContent("weather");
    expect(screen.queryByRole("textbox", { name: "weather" })).not.toBeInTheDocument();
    expect(screen.getByText(en.models.enumValues.unused)).toBeInTheDocument();

    const box = screen.getByRole("textbox", { name: en.models.enumValues.new });
    expect(box).toHaveAccessibleDescription(en.models.enumValues.newHint);
    await userEvent.type(box, "fire");
    await openEnum("Severity");
    expect(screen.getByRole("textbox", { name: en.models.enumValues.new })).toHaveValue("");
    await openEnum("AlertCategory");
    await userEvent.type(screen.getByRole("textbox", { name: en.models.enumValues.new }), "fire{Enter}");
    expect(values(source()).map((v) => v.name)).toContain("fire");
    expect(screen.getByRole("textbox", { name: en.models.enumValues.new })).toHaveValue("");
  });

  it("edits a value's label and its translation, and removes a value", async () => {
    await openEnum("AlertCategory");
    await userEvent.type(screen.getByRole("textbox", { name: en.models.enumValues.labelOf.replace("{value}", "weather") }), "Weather");
    expect(values(source())[1].title).toEqual({ en: "Weather" });
    const sk = document.getElementById("enum-AlertCategory-weather-title-sk") as HTMLInputElement;
    await userEvent.type(sk, "Počasie");
    expect(values(source())[1].title).toEqual({ en: "Weather", sk: "Počasie" });
    await userEvent.click(screen.getByRole("button", { name: en.models.removeEnumValue.replace("{value}", "weather") }));
    expect(values(source()).map((value) => value.name)).toEqual(["traffic", "health"]);
  });
});

describe("enumUses", () => {
  it("names each field whose range is the enumeration, with the types that carry it", () => {
    const model = parseModel(`id: https://hel.fi/m
name: m
classes:
  Alert: { slots: [category] }
  Notice: { slots: [category, level] }
slots:
  category: { range: AlertCategory }
  level: { range: Severity }
enums:
  AlertCategory: { permissible_values: { traffic: {} } }
`);
    expect(enumUses(model, "AlertCategory")).toEqual([{ field: "category", types: ["Alert", "Notice"] }]);
    expect(enumUses(model, "Nothing")).toEqual([]);
  });
});

describe("renaming an import before Save (T-6353)", () => {
  const source = [
    "id: https://example.org/air",
    "name: air",
    "classes:",
    "  Air:",
    "    slots: [level, status]",
    "  Reading:",
    "    is_a: Air",
    "slots:",
    "  level:",
    "    range: Level",
    "  status:",
    "    range: Air",
    "enums:",
    "  Level:",
    "    permissible_values:",
    "      low:",
    "        title: Low",
    "      high: {}",
    "",
  ].join("\n");
  const one = (operation: Operation) => {
    const applied = applyOperations(source, [operation]);
    return { model: parseModel(applied.source), reason: applied.refused[0]?.reason };
  };

  it("renames the type, in the class that specialises it and the slot that ranges over it", () => {
    const { model } = one({ op: "renameClass", name: "Air", to: "AirReading" });
    expect(model.classes.map((klass) => klass.name)).toContain("AirReading");
    expect(model.classes.find((klass) => klass.name === "Reading")?.is_a).toBe("AirReading");
    expect(model.slots.find((slot) => slot.name === "status")?.range).toBe("AirReading");
  });

  it("renames a field in every class that lists it", () => {
    const { model } = one({ op: "renameSlot", name: "level", to: "airLevel" });
    expect(model.classes.find((klass) => klass.name === "Air")?.slots).toEqual(["airLevel", "status"]);
  });

  it("renames an enumeration and the field that uses it", () => {
    const { model } = one({ op: "renameEnum", name: "Level", to: "AirLevel" });
    expect(model.enums.map((entry) => entry.name)).toEqual(["AirLevel"]);
    expect(model.slots.find((slot) => slot.name === "level")?.range).toBe("AirLevel");
  });

  it("renames a stored value and keeps its label", () => {
    const { model } = one({ op: "renameEnumValue", enum: "Level", value: "low", to: "minimal" });
    const values = model.enums[0].permissible_values;
    expect(values.map((value) => value.name)).toEqual(["minimal", "high"]);
    expect(values[0].title?.en).toBe("Low");
  });

  it("refuses a name another of its kind has, and an empty value", () => {
    expect(one({ op: "renameEnumValue", enum: "Level", value: "low", to: "high" }).reason).toContain("already has");
    expect(one({ op: "renameEnumValue", enum: "Level", value: "low", to: " " }).reason).toContain("needs a name");
    expect(one({ op: "renameEnum", name: "Level", to: "Level" }).reason).toContain("already exists");
    expect(one({ op: "renameEnum", name: "Nothing", to: "Other" }).reason).toContain("unknown enum");
  });
});

describe("NameField", () => {
  it("applies on Enter, and goes back to the name when the model refuses", () => {
    const onRename = vi.fn(() => false);
    render(<NameField id="n" label="Name" help="For example Air" name="Air" onRename={onRename} />);
    const box = screen.getByRole("textbox", { name: /Name/ });
    fireEvent.change(box, { target: { value: "Water" } });
    expect(onRename).not.toHaveBeenCalled();
    fireEvent.keyDown(box, { key: "Enter" });
    expect(onRename).toHaveBeenCalledWith("Water");
    expect(box).toHaveValue("Air");
  });

  it("renames nothing on leaving an empty or unchanged box", () => {
    const onRename = vi.fn(() => true);
    render(<NameField id="n" label="Name" help="For example Air" name="Air" onRename={onRename} />);
    const box = screen.getByRole("textbox", { name: /Name/ });
    fireEvent.blur(box);
    fireEvent.change(box, { target: { value: "" } });
    fireEvent.blur(box);
    expect(onRename).not.toHaveBeenCalled();
    expect(box).toHaveValue("Air");
  });
});
