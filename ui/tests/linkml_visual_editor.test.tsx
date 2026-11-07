/** T-0218: authoring classes, slots and enums without leaving the document (DM-04…DM-06, DM-13, DM-16). */
import en from "../src/locales/en.json";
import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";
import { LinkmlVisualEditor, TitleRow } from "../src/pages/models/LinkmlVisualEditor";
import { blankSource, diagnose, parseModel } from "../src/pages/models/linkml";

const SOURCE = `# the model the city publishes
id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots:
      - pm10
slots:
  pm10:
    range: float
    slot_uri: bb:pm10
enums: {}
`;

/** The editor is controlled, so the test holds the one document both views would share. */
function Harness({ initial = SOURCE }: { initial?: string }) {
  const [source, setSource] = useState(initial);
  return (
    <>
      <LinkmlVisualEditor
        source={source}
        onChange={setSource}
        diagnostics={diagnose(source, ["sk", "en"])}
        locales={["sk", "en"]}
      />
      <textarea readOnly aria-label="source" value={source} />
    </>
  );
}

function renderEditor(initial?: string) {
  render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return {
    source: () => (screen.getByLabelText("source") as HTMLTextAreaElement).value,
    user: userEvent.setup(),
  };
}

describe("LinkML visual editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("lists the parent's fields greyed, with the type they come from (T-6106)", async () => {
    const { user } = renderEditor(
      SOURCE.replace(
        "  AirQualityObserved:\n    class_uri: bb:AirQualityObserved\n",
        "  Observed:\n    slots:\n      - dateObserved\n  AirQualityObserved:\n    is_a: Observed\n    class_uri: bb:AirQualityObserved\n",
      ).replace("slots:\n  pm10:", "slots:\n  dateObserved:\n    range: datetime\n  pm10:"),
    );

    await user.click(screen.getByRole("button", { name: "AirQualityObserved" }));
    const fields = screen.getByRole("table", { name: en.models.slots });
    const inherited = within(fields).getByRole("button", { name: "dateObserved" }).closest("tr")!;
    expect(inherited).toHaveClass("[&>td]:bg-surface-muted");
    expect(inherited).not.toHaveAttribute("aria-current");
    // Once as the From column's button, once under the name for a phone, which hides that column.
    expect(within(inherited).getAllByText(i18n.t("models.inheritedFrom", { type: "Observed" }))).toHaveLength(2);
    const own = within(fields).getByRole("button", { name: "pm10" }).closest("tr")!;
    expect(own).not.toHaveClass("[&>td]:bg-surface-muted");
    expect(within(own).getByText(en.models.ownField)).toBeInTheDocument();
    await user.click(within(inherited).getByRole("button", { name: i18n.t("models.inheritedFrom", { type: "Observed" }) }));
    expect(screen.getByRole("button", { name: "Observed" })).toHaveAttribute("aria-current", "true");
  });

  it("offers a type picker for a phone, where the rail sits under the type it opens (T-6106)", async () => {
    const { user } = renderEditor(
      SOURCE.replace("  AirQualityObserved:\n", "  Observed:\n    slots:\n      - pm10\n  AirQualityObserved:\n"),
    );
    const picker = screen.getByRole("combobox", { name: en.models.typePicker });
    expect(picker.closest(".lg\\:hidden")).not.toBeNull();
    await user.selectOptions(picker, "AirQualityObserved");
    expect(screen.getByRole("button", { name: "AirQualityObserved" })).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("heading", { name: "AirQualityObserved", level: 2 })).toBeInTheDocument();
  });

  it("opens an inherited field read-only, naming the parent it is defined on, and goes there (T-6106)", async () => {
    const { source, user } = renderEditor(
      SOURCE.replace(
        "  AirQualityObserved:\n    class_uri: bb:AirQualityObserved\n",
        "  Observed:\n    slots:\n      - dateObserved\n  AirQualityObserved:\n    is_a: Observed\n    class_uri: bb:AirQualityObserved\n",
      ).replace("slots:\n  pm10:", "slots:\n  dateObserved:\n    range: datetime\n  pm10:"),
    );
    const before = source();

    await user.click(screen.getByRole("button", { name: "AirQualityObserved" }));
    await user.click(within(screen.getByRole("table", { name: en.models.slots })).getByRole("button", { name: "dateObserved" }));
    // The opened row is marked under the field's dialog (owner 2026-11-01: a field opens as a dialog).
    expect(within(screen.getByRole("table", { name: en.models.slots, hidden: true })).getByRole("button", { name: "dateObserved", hidden: true }).closest("tr")).toHaveAttribute("aria-current", "true");
    const detail = screen.getByRole("dialog", { name: "dateObserved" });
    expect(within(detail).getByText(i18n.t("models.inheritedDefinedOn", { type: "Observed" }))).toBeInTheDocument();
    // Nothing to edit here: a change would reach every type that inherits the field.
    expect(within(detail).queryByRole("checkbox")).not.toBeInTheDocument();
    expect(within(detail).queryByRole("combobox")).not.toBeInTheDocument();
    expect(source()).toBe(before);

    await user.click(within(detail).getByRole("button", { name: i18n.t("models.inheritedOpen", { type: "Observed" }) }));
    expect(screen.getByRole("button", { name: "Observed", hidden: true })).toHaveAttribute("aria-current", "true");
    // On its own type the field is editable again.
    expect(within(screen.getByRole("dialog", { name: "dateObserved" })).getAllByRole("checkbox").length).toBeGreaterThan(0);
  });

  it("keeps a phone page 375 px wide: the attribute table scrolls in its own frame, the editor's column does not grow to it (T-4165)", () => {
    renderEditor();
    const frame = screen.getByRole("group", { name: en.models.slots });
    // The outermost grid of the editor: its one column below lg is minmax(0, 1fr), never auto.
    let grid: HTMLElement | null = frame.parentElement;
    while (grid && !grid.className.split(" ").includes("grid")) grid = grid.parentElement;
    while (grid?.parentElement?.closest(".grid")) grid = grid.parentElement.closest(".grid") as HTMLElement;
    expect(grid).toHaveClass("grid-cols-1");
    expect(grid?.className).not.toMatch(/grid-cols-\[[^\]]*_1fr\]/);
  });

  it("creates a class, and the YAML keeps the comment that was already there", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "Add type" }));
    await user.type(screen.getByRole("textbox", { name: "New type" }), "WeatherObserved");
    await user.click(screen.getByRole("button", { name: "Add" }));

    expect(parseModel(source()).classes.map((klass) => klass.name)).toEqual([
      "AirQualityObserved",
      "WeatherObserved",
    ]);
    expect(source()).toContain("# the model the city publishes");
    expect(await screen.findByRole("button", { name: "WeatherObserved" })).toBeInTheDocument();
  });

  it("makes a new type an NGSI-LD entity type when the model imports ngsi-ld-core, so Check finds one (T-5066)", async () => {
    const { source, user } = renderEditor(blankSource("hel.fi", "stations"));

    await user.click(screen.getByRole("button", { name: "Add type" }));
    await user.type(screen.getByRole("textbox", { name: "New type" }), "Station");
    await user.click(screen.getByRole("button", { name: "Add" }));

    expect(parseModel(source()).classes.find((klass) => klass.name === "Station")?.is_a).toBe("Entity");
  });

  it("leaves a new type without a parent in a model that has no Entity to specialise", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "Add type" }));
    await user.type(screen.getByRole("textbox", { name: "New type" }), "WeatherObserved");
    await user.click(screen.getByRole("button", { name: "Add" }));

    expect(parseModel(source()).classes.find((klass) => klass.name === "WeatherObserved")?.is_a).toBeUndefined();
  });

  it("adds a slot to the open class and lists it with its range", async () => {
    const { source, user } = renderEditor();

    const slots = screen.getByRole("table");
    await user.click(screen.getByRole("button", { name: "Add field" }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "pm25");
    await user.click(screen.getByRole("button", { name: "Add" }));

    const model = parseModel(source());
    expect(model.classes[0].slots).toEqual(["pm10", "pm25"]);
    expect(model.slots.find((slot) => slot.name === "pm25")?.range).toBe("string");
    // The new field opens in its dialog; the table under it lists both.
    expect(screen.getByRole("dialog", { name: "pm25" })).toBeInTheDocument();
    expect(within(slots).getByRole("row", { name: /pm10/, hidden: true })).toBeInTheDocument();
  });

  it("edits a slot's range, kind, unit and required flag through the document", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.selectOptions(screen.getByLabelText("Type of value"), "integer");
    await user.selectOptions(screen.getByLabelText("NGSI-LD kind"), "GeoProperty");
    // The unit is searched, not scrolled for (T-2809): the UCUM spelling finds µg/m³.
    await user.type(screen.getByRole("combobox", { name: "Unit" }), "ug/m3");
    await user.keyboard("{Enter}");
    await user.click(screen.getByLabelText("Required"));

    const slot = parseModel(source()).slots.find((candidate) => candidate.name === "pm10");
    expect(slot?.range).toBe("integer");
    expect(slot?.kind).toBe("GeoProperty");
    expect(slot?.required).toBe(true);
    expect(slot?.unit?.ucum_code).toBe("ug.m-3");
    // DM-06: the CEFACT common code travels with the unit, not only the UCUM symbol.
    expect(slot?.unit?.exact_mappings).toEqual(["ucefact:GQ", "qudt-unit:MicroGM-PER-M3"]);
    // DM-05: the kind is the annotation Model Tools reads, not a field of our own.
    expect(source()).toContain("ngsi_ld_kind: GeoProperty");
  });

  /// T-6101: a unit measures a number. Picking one on a text field makes it a Number (an
  /// Integer for C62) and says so; leaving numbers drops the unit; Undo takes back both changes.
  it("turns a text field into a number when it gets a unit, and Undo takes both back", async () => {
    const { source, user } = renderEditor(SOURCE.replace("range: float", "range: string"));
    const pm10 = () => parseModel(source()).slots.find((candidate) => candidate.name === "pm10");

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.type(screen.getByRole("combobox", { name: "Unit" }), "CEL");
    await user.keyboard("{Enter}");
    expect(pm10()?.range).toBe("float");
    expect(pm10()?.unit?.exact_mappings).toContain("ucefact:CEL");
    expect(screen.getByRole("status")).toHaveTextContent(en.models.unitRule.madeNumber);
    expect(screen.getByRole("row", { name: /pm10/, hidden: true })).toHaveTextContent("°C");

    await user.click(screen.getByRole("button", { name: en.models.unitRule.undo }));
    expect(pm10()?.range).toBe("string");
    expect(pm10()?.unit).toBeUndefined();
    expect(screen.queryByText(en.models.unitRule.madeNumber)).not.toBeInTheDocument();
  });

  /// Undo puts back the text from before the pick, so it goes away once anything else changed.
  it("drops the unit Undo after a later edit, keeping that edit", async () => {
    const { source, user } = renderEditor(SOURCE.replace("range: float", "range: string"));

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.type(screen.getByRole("combobox", { name: "Unit" }), "CEL");
    await user.keyboard("{Enter}");
    expect(screen.getByRole("button", { name: en.models.unitRule.undo })).toBeInTheDocument();

    // A later edit in the same field dialog (owner 2026-11-01: the dialog covers the type's form).
    await user.click(screen.getByLabelText("Required"));
    expect(screen.queryByRole("button", { name: en.models.unitRule.undo })).not.toBeInTheDocument();
    const pm10 = parseModel(source()).slots.find((candidate) => candidate.name === "pm10");
    expect(pm10?.required).toBe(true);
    expect(pm10?.range).toBe("float");
  });

  it("makes a count an Integer, and drops the unit of a field that stops being a number", async () => {
    const { source, user } = renderEditor(SOURCE.replace("range: float", "range: string"));
    const pm10 = () => parseModel(source()).slots.find((candidate) => candidate.name === "pm10");

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.type(screen.getByRole("combobox", { name: "Unit" }), "C62");
    await user.keyboard("{Enter}");
    expect(pm10()?.range).toBe("integer");
    expect(screen.getByRole("status")).toHaveTextContent(en.models.unitRule.madeInteger);

    await user.selectOptions(screen.getByLabelText("Type of value"), "string");
    expect(pm10()?.range).toBe("string");
    expect(pm10()?.unit).toBeUndefined();
    expect(screen.getByRole("status")).toHaveTextContent(en.models.unitRule.droppedUnit);

    await user.click(screen.getByRole("button", { name: en.models.unitRule.undo }));
    expect(pm10()?.range).toBe("integer");
    expect(pm10()?.unit?.exact_mappings).toContain("ucefact:C62");
  });

  /**
   * T-1088, DM-13: the hierarchy the model declares is edited where it is read. The parent is a
   * choice among the model's own classes, so a typo cannot leave a dangling reference behind.
   */
  it("sets a class's parent and mixins, and a slot's profiles, through the document", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "AirQualityObserved" }));
    // A class to specialise has to exist before it can be chosen.
    expect(screen.getByLabelText("The class it specialises")).toBeInTheDocument();
    expect(
      within(screen.getByLabelText("The class it specialises")).queryByRole("option", {
        name: "AirQualityObserved",
      }),
      "a class is never offered itself as its own parent",
    ).toBeNull();

    await user.type(screen.getByLabelText("Mixed in (comma separated)"), "AirQualityObserved");
    // Itself is refused, so the document is untouched and no mixins key appears.
    expect(source()).not.toContain("mixins:");

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.type(screen.getByLabelText("Profiles (comma separated)"), "public");
    expect(
      parseModel(source()).slots.find((candidate) => candidate.name === "pm10")?.subsets,
    ).toEqual(["public"]);
  });

  it("refuses to mint a slot IRI under a namespace that belongs to someone else", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.clear(screen.getByLabelText("Slot IRI"));
    await user.type(screen.getByLabelText("Slot IRI"), "sdm:pm10");

    expect(await screen.findByRole("alert")).toHaveTextContent("smartdatamodels.org");
    // The refused term never reaches the document, and the field keeps what was typed so the
    // person can correct it rather than watch keystrokes disappear.
    expect(source()).not.toContain("sdm:pm10");
    expect(screen.getByLabelText("Slot IRI")).toHaveValue("sdm:pm10");
  });

  it("lets an upstream slot keep the upstream IRI it cites", async () => {
    const cited = SOURCE.replace(
      "    slot_uri: bb:pm10",
      "    slot_uri: sdm:pm10\n    annotations:\n      upstream_source: https://github.com/smart-data-models/dataModel.Environment@9f1c2b7",
    );
    const { source, user } = renderEditor(cited);

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.clear(screen.getByLabelText("Slot IRI"));
    await user.type(screen.getByLabelText("Slot IRI"), "sdm:pm10");

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(parseModel(source()).slots[0].slot_uri).toBe("sdm:pm10");
  });

  // T-6254 judge: three empty Title inputs filled a phone's first screen.
  it("shows one Title row in the reader's language and the other locales behind Translations", async () => {
    const { source, user } = renderEditor();
    expect(screen.getByRole("textbox", { name: "Title (en)" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Title (en)" })).toHaveAccessibleDescription(en.models.titleHint);
    const translations = screen.getByText(en.models.titleTranslations.replace("{languages}", "sk"));
    const disclosure = translations.closest("details") as HTMLDetailsElement;
    expect(disclosure.open).toBe(false);
    expect(disclosure).toContainElement(screen.getByLabelText("Title (sk)"));
    await user.click(translations);
    expect(disclosure.open).toBe(true);
    await user.type(screen.getByLabelText("Title (sk)"), "Ovzdušie");
    expect(parseModel(source()).classes[0].title).toEqual({ sk: "Ovzdušie" });
  });

  it("TitleRow puts the first locale first when the reader's language is not configured", async () => {
    const onChange = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <TitleRow locales={["fi", "sk"]} title={{ fi: "Ilma" }} onChange={onChange} />
      </I18nextProvider>,
    );
    expect(screen.getByRole("textbox", { name: "Title (fi)" })).toHaveValue("Ilma");
    expect(screen.getByLabelText("Title (sk)").closest("details")).not.toBeNull();
    await userEvent.setup().type(screen.getByRole("textbox", { name: "Title (fi)" }), "!");
    expect(onChange).toHaveBeenLastCalledWith("fi", "Ilma!");
  });

  it("offers no New field before the model has a type, and says to add one first (T-6254)", () => {
    renderEditor("id: https://example.org/m\nname: m\nclasses: {}\nslots: {}\n");
    expect(screen.getByText(en.models.fieldsNeedType)).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: en.models.newSlot })).not.toBeInTheDocument();
  });

  it("writes a title per configured locale, and warns while one is missing", async () => {
    const { source, user } = renderEditor();

    await user.type(screen.getByLabelText("Title (sk)"), "Kvalita ovzdušia");

    expect(parseModel(source()).classes[0].title).toEqual({ sk: "Kvalita ovzdušia" });
    const warning = await screen.findByText(/has no title in en/, { selector: "p" });
    expect(warning).toBeInTheDocument();
    // In the warning tone, and in a size the theme defines: `text-warning-fg` is a name the
    // theme has for `danger` and `primary` but not for `warning`, so Tailwind emitted nothing
    // and the diagnostic read as ordinary text (T-2422, UI-30).
    expect(warning.className).toContain("text-warning");
    expect(warning.className).toMatch(/\btext-(caption|body)\b/);
  });

  it("adds an enum and a value to it", async () => {
    const { source, user } = renderEditor();

    await user.type(screen.getByLabelText("New enum"), "QualityBand");
    await user.click(screen.getByRole("button", { name: "Add enum" }));

    await user.type(await screen.findByRole("textbox", { name: "New value" }), "good");
    await user.click(screen.getByRole("button", { name: "Add value to QualityBand" }));

    const model = parseModel(source());
    expect(model.enums[0].name).toBe("QualityBand");
    expect(model.enums[0].permissible_values.map((value) => value.name)).toEqual(["good"]);
  });
});

/**
 * The UI contract of the structure view (T-1772, UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The three flags of a slot were hand-made `<input type="checkbox">` inside a bare `<label>`,
 * the class list was a hand-made `<button>` and the parent a hand-made `<select>`; each is now
 * the shared control, which is what gives them one focus ring, one disabled state and a name a
 * pointer can hit. These tests hold that: axe over the class and the slot open, every control
 * reachable in DOM order, and the four locales on the labels.
 */
describe("the LinkML visual editor against the UI contract", () => {
  /** The editor alone, over the one document it edits: no test widget beside it for axe to read. */
  function Alone({ initial }: { initial: string }) {
    const [source, setSource] = useState(initial);
    return (
      <LinkmlVisualEditor
        source={source}
        onChange={setSource}
        diagnostics={diagnose(source, ["sk", "en"])}
        locales={["sk", "en"]}
      />
    );
  }

  function renderAlone(initial = SOURCE) {
    const view = render(
      <I18nextProvider i18n={i18n}>
        <Alone initial={initial} />
      </I18nextProvider>,
    );
    return { container: view.container, user: userEvent.setup() };
  }

  it("has no axe violation with a class open and with a slot open", async () => {
    await i18n.changeLanguage("en");
    const { container, user } = renderAlone();

    await expectNoViolations(container);

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await screen.findByLabelText("Slot IRI");
    await expectNoViolations(container);
  });

  it("ticks a flag of a slot by its own words, and says which flag it is", async () => {
    await i18n.changeLanguage("en");
    const { user } = renderAlone();
    await user.click(screen.getByRole("button", { name: "pm10" }));

    // The label wraps the box, so the words beside it are the control's name and its hit area.
    const required = screen.getByRole("checkbox", { name: "Required" });
    expect(required).not.toBeChecked();
    await user.click(screen.getByText("Required"));
    expect(required).toBeChecked();

    expect(screen.getByRole("checkbox", { name: "Multivalued" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /Deprecated/ })).toBeInTheDocument();
  });

  it("reaches every control of the open slot by keyboard in the order it is read", async () => {
    await i18n.changeLanguage("en");
    const { user } = renderAlone();
    await user.click(screen.getByRole("button", { name: "pm10" }));

    const panel = screen.getByLabelText("Slot IRI").closest("section, div[class*=flex-col]");
    expect(panel).not.toBeNull();
    await expectTabOrder(user, panel as HTMLElement);
  });

  it("offers the parent class as a choice, not as free text a typo breaks", async () => {
    await i18n.changeLanguage("en");
    const { user } = renderAlone(
      SOURCE.replace(
        "slots:\n  pm10:",
        "  Station:\n    class_uri: bb:Station\n    slots: []\nslots:\n  pm10:",
      ),
    );
    await user.click(screen.getByRole("button", { name: "AirQualityObserved" }));

    const parent = screen.getByLabelText("The class it specialises");
    expect(parent.tagName).toBe("SELECT");
    expect(within(parent).getByRole("option", { name: "Station" })).toBeInTheDocument();
  });

  it.each(SUPPORTED_LOCALES)("labels the slot's flags in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container, user } = renderAlone();
    await user.click(screen.getByRole("button", { name: "pm10" }));

    for (const key of ["models.required", "models.multivalued", "models.deprecatedKeep"]) {
      expect(
        screen.getByRole("checkbox", { name: i18n.t(key) }),
        `${key} has a ${locale} label`,
      ).toBeInTheDocument();
    }
    expectNoRawKeys(container);
  });
});
