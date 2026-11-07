/** T-0575: a subset of the model is what an endpoint exposes (MP-01, MP-03, UI-43). */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied } from "./checks";
import { ModelSubsetPicker } from "../src/pages/models/ModelSubsetPicker";
import { parseModel, effectiveSlots, slotOrigins } from "../src/pages/models/linkml";
import { EMPTY_SUBSET, subsetProblems, subsetSource, wholeSubset } from "../src/pages/models/subset";
import type { Subset } from "../src/pages/models/subset";

const SOURCE = `id: https://hel.fi/models/fleet
name: fleet
prefixes:
  hel: https://hel.fi/terms/
classes:
  Vehicle:
    class_uri: hel:Vehicle
    slots: [id, type, name, category, speed]
  User:
    class_uri: hel:User
    slots: [id, type, name, age]
slots:
  id:
    identifier: true
  type:
    designates_type: true
  name:
    range: string
  category:
    range: VehicleCategory
  speed:
    range: float
  age:
    range: integer
enums:
  VehicleCategory:
    permissible_values:
      public: {}
      private: {}
`;

const MODEL = parseModel(SOURCE);

function Harness({ initial = EMPTY_SUBSET }: { initial?: Subset }) {
  const [subset, setSubset] = useState<Subset>(initial);
  return (
    <>
      <ModelSubsetPicker model={MODEL} subset={subset} onChange={setSubset} />
      <pre aria-label="subset">{JSON.stringify(subset)}</pre>
    </>
  );
}

function renderPicker(initial?: Subset) {
  render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return {
    subset: () => JSON.parse(screen.getByLabelText("subset").textContent ?? "") as Subset,
    user: userEvent.setup(),
  };
}

describe("model subset", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("ticking Vehicle(id, name, type) and User(id, name, type, age) yields exactly that", async () => {
    const { subset, user } = renderPicker();
    expect(screen.getByRole("status")).toHaveTextContent("Nothing is ticked");

    await user.click(screen.getByRole("checkbox", { name: "Vehicle.name" }));
    await user.click(screen.getByRole("checkbox", { name: "User" }));
    await user.click(screen.getByRole("checkbox", { name: "User.name" }));
    await user.click(screen.getByRole("checkbox", { name: "User.age" }));

    expect(subset()).toEqual({
      classes: [
        { name: "Vehicle", slots: ["name"] },
        { name: "User", slots: ["name", "age"] },
      ],
    });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    // Ticking a slot ticked its class; the identity slots came with it and cannot go.
    expect(screen.getByRole("checkbox", { name: "Vehicle" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Vehicle.id" })).toBeChecked();
    // Refused and still reachable, so the reason can be read (T-1743, UI-44).
    expectDenied(screen.getByRole("checkbox", { name: "Vehicle.id" }), en.models.subset.identity);
    expectDenied(screen.getByRole("checkbox", { name: "Vehicle.type" }), en.models.subset.identity);
  });

  it("distinguishes a class ticked with no slots from one not ticked", async () => {
    const { subset, user } = renderPicker(wholeSubset(MODEL));
    expect(subset().classes).toHaveLength(2);

    await user.click(screen.getByRole("checkbox", { name: "User.name" }));
    await user.click(screen.getByRole("checkbox", { name: "User.age" }));
    expect(subset().classes.find((klass) => klass.name === "User")).toEqual({ name: "User", slots: ["id", "type"] });

    await user.click(screen.getByRole("checkbox", { name: "User" }));
    expect(subset().classes.map((klass) => klass.name)).toEqual(["Vehicle"]);
    expect(screen.getByRole("checkbox", { name: "User.id" })).not.toBeChecked();
  });

  it("narrows the source to the chosen classes, slots and the enums they still use", () => {
    const narrowed = parseModel(
      subsetSource(SOURCE, {
        classes: [
          { name: "Vehicle", slots: ["name"] },
          { name: "User", slots: ["name", "age"] },
        ],
      }),
    );
    expect(narrowed.classes.map((klass) => [klass.name, klass.slots])).toEqual([
      ["Vehicle", ["id", "type", "name"]],
      ["User", ["id", "type", "name", "age"]],
    ]);
    expect(narrowed.slots.map((slot) => slot.name).sort()).toEqual(["age", "id", "name", "type"]);
    expect(narrowed.enums).toEqual([]);
    expect(narrowed.prefixes.hel).toBe("https://hel.fi/terms/");

    const withEnum = parseModel(subsetSource(SOURCE, { classes: [{ name: "Vehicle", slots: ["category"] }] }));
    expect(withEnum.classes).toHaveLength(1);
    expect(withEnum.enums.map((entry) => entry.name)).toEqual(["VehicleCategory"]);

    expect(parseModel(subsetSource(SOURCE, EMPTY_SUBSET)).classes).toEqual([]);
  });

  it("names every class and slot the model does not have", () => {
    expect(
      subsetProblems(MODEL, {
        classes: [
          { name: "Vehicle", slots: ["name", "colour"] },
          { name: "Bicycle", slots: ["id"] },
          { name: "Vehicle", slots: [] },
        ],
      }),
    ).toEqual([
      "class 'Vehicle' has no slot 'colour'",
      "unknown class 'Bicycle'",
      "class 'Vehicle' is listed twice",
    ]);
    render(
      <I18nextProvider i18n={i18n}>
        <ModelSubsetPicker
          model={MODEL}
          subset={{ classes: [{ name: "Bicycle", slots: [] }] }}
          onChange={() => {}}
        />
      </I18nextProvider>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("unknown class 'Bicycle'");
  });
});

/**
 * T-1112, DM-13: a class carries the slots its parent and its mixins bring, so an endpoint
 * projecting by class must see them. The picker showed only the line the class declared itself,
 * which is not what the entity carries.
 */
describe("the slots a class actually carries", () => {
  const model = parseModel(`
id: https://hel.fi/models/air
name: air
classes:
  Entity:
    slots: [id_, createdAt]
  Timed:
    slots: [observedAt]
  AirQualityObserved:
    is_a: Entity
    mixins: [Timed]
    slots: [pm10]
slots:
  id_: {}
  createdAt: {}
  observedAt: {}
  pm10: {}
`);

  it("answers the parent's and the mixins' slots before its own", () => {
    const klass = model.classes.find((one) => one.name === "AirQualityObserved");
    expect(effectiveSlots(model, klass!)).toEqual(["id_", "createdAt", "observedAt", "pm10"]);
  });

  it("names the class each slot comes from, the class itself for one it redeclares (T-6106)", () => {
    const klass = model.classes.find((one) => one.name === "AirQualityObserved")!;
    expect([...slotOrigins(model, klass)]).toEqual([
      ["id_", "Entity"],
      ["createdAt", "Entity"],
      ["observedAt", "Timed"],
      ["pm10", "AirQualityObserved"],
    ]);
    const redeclared = { ...klass, slots: ["pm10", "createdAt"] };
    expect(slotOrigins(model, redeclared).get("createdAt")).toBe("AirQualityObserved");
  });

  it("answers a class with no hierarchy exactly its own slots", () => {
    const entity = model.classes.find((one) => one.name === "Entity");
    expect(effectiveSlots(model, entity!)).toEqual(["id_", "createdAt"]);
  });

  it("walks a hierarchy that loops once and no further", () => {
    const looping = parseModel(`
id: https://hel.fi/models/loop
name: loop
classes:
  A:
    is_a: B
    slots: [a]
  B:
    is_a: A
    slots: [b]
slots:
  a: {}
  b: {}
`);
    const a = looping.classes.find((one) => one.name === "A");
    expect(effectiveSlots(looping, a!).sort()).toEqual(["a", "b"]);
  });

  it("ignores a parent the model does not declare rather than failing", () => {
    const dangling = parseModel(`
id: https://hel.fi/models/dangling
name: dangling
classes:
  A:
    is_a: Nowhere
    slots: [a]
slots:
  a: {}
`);
    const a = dangling.classes.find((one) => one.name === "A");
    expect(effectiveSlots(dangling, a!)).toEqual(["a"]);
  });
});
