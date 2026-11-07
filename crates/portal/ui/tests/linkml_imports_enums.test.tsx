/**
 * T-2720 (DM-13, DM-61, UI-84): the model as LinkML writes it and as a Smart Data Model ships
 * it. Inline `attributes` and `slot_usage` are read, `imports` are resolved against the
 * project's models, enums are drawn as boxes with their values, and an empty model shows three
 * ways to start.
 */
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import { LinkmlEditor } from "../src/pages/models/LinkmlEditor";
import { LinkmlVisualEditor } from "../src/pages/models/LinkmlVisualEditor";
import { classSlots, graphData, importName, parseModel, withImports } from "../src/pages/models/linkml";
import { enumsOfModel, filterSlotsOf } from "../src/components/entities/filters";
import { renderPart } from "./page_contract";
import { jsonResponse, list, renderRoute } from "./pageHarness";

const SCHOOL = `# the city's schools
name: schools
imports:
  - linkml:types
  - ./people.linkml.yaml
classes:
  School:
    slots: [name, level]
    slot_usage:
      name:
        required: true
    attributes:
      capacity:
        range: integer
      principal:
        range: Person
      name:
        range: integer
slots:
  name: { range: string }
  level: { range: SchoolLevel }
enums:
  SchoolLevel:
    permissible_values:
      primary: { title: { en: Primary } }
      secondary: {}
`;

const PEOPLE = `name: people
classes:
  Person:
    slots: [givenName]
  School:
    slots: [givenName]
slots:
  givenName: { range: string }
enums:
  Role:
    permissible_values:
      teacher: {}
`;

const opening = (name: string) => en.models.graph.openClass.replace("{name}", name);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("a class as LinkML writes it", () => {
  it("reads inline attributes and slot_usage, the listed slot winning a name both carry", () => {
    const model = parseModel(SCHOOL);
    const school = model.classes[0];
    // `slots` stays the list an operation writes back: the attributes are kept apart.
    expect(school.slots).toEqual(["name", "level"]);
    expect(classSlots(model, school).map((slot) => [slot.name, slot.range, slot.required])).toEqual([
      ["name", "string", true],
      ["level", "SchoolLevel", false],
      ["capacity", "integer", false],
      ["principal", "Person", false],
    ]);
  });

  it("leaves a listed slot the model does not declare out, and a class without extras as it was", () => {
    const model = parseModel("classes:\n  A:\n    slots: [ghost, x]\nslots:\n  x: { range: string }\n");
    expect(classSlots(model, model.classes[0]).map((slot) => slot.name)).toEqual(["x"]);
    expect(model.classes[0].attributes).toEqual([]);
    expect(model.classes[0].slot_usage).toEqual({});
  });

  it("offers the inline attributes to the grid's filters and enum pickers", () => {
    expect(filterSlotsOf(SCHOOL, "School").map((slot) => slot.name)).toEqual(["name", "level", "capacity", "principal"]);
    expect(enumsOfModel(SCHOOL, "School", "en")).toEqual({
      level: [
        { value: "primary", title: "Primary", description: undefined },
        { value: "secondary", title: undefined, description: undefined },
      ],
    });
  });
});

describe("imports", () => {
  it("name a model of ours, never the metamodel's own schemas", () => {
    expect(importName("./people.linkml.yaml")).toBe("people");
    expect(importName("people")).toBe("people");
    expect(importName("https://hel.fi/models/people.yaml")).toBe("people");
    expect(importName("linkml:types")).toBeUndefined();
    expect(importName("")).toBeUndefined();
    expect(importName("./")).toBeUndefined();
  });

  it("bring in what the model does not declare, marked with where it came from", () => {
    const merged = withImports(parseModel(SCHOOL), { people: parseModel(PEOPLE) });
    expect(merged.classes.map((klass) => [klass.name, klass.from])).toEqual([
      ["School", undefined],
      ["Person", "people"],
    ]);
    expect(merged.enums.map((entry) => [entry.name, entry.from])).toEqual([
      ["SchoolLevel", undefined],
      ["Role", "people"],
    ]);
    expect(merged.slots.map((slot) => slot.name)).toEqual(["name", "level", "givenName"]);
  });
});

describe("the diagram", () => {
  it("draws enums as boxes with their values, and a line to them from the slot that picks", () => {
    const { nodes, edges } = graphData(withImports(parseModel(SCHOOL), { people: parseModel(PEOPLE) }));
    expect(nodes.map((node) => [node.name, node.kind, node.depth, node.from])).toEqual([
      ["School", "class", 0, undefined],
      ["Person", "class", 0, "people"],
      ["SchoolLevel", "enum", 1, undefined],
      ["Role", "enum", 1, "people"],
    ]);
    expect(nodes[0].slots).toEqual(["name", "level", "capacity", "principal"]);
    expect(nodes[2].slots).toEqual(["primary", "secondary"]);
    expect(edges).toEqual([
      { from: "School", to: "SchoolLevel", kind: "enum", label: "level" },
      { from: "School", to: "Person", kind: "range", label: "principal", fromMultiplicity: "*", toMultiplicity: "0..1" },
    ]);
  });

  it("names an enum box for a screen reader and keeps it out of the tab order", async () => {
    const onOpenClass = vi.fn();
    renderPart(<LinkmlGraphView source={SCHOOL} imports={{ people: PEOPLE }} onOpenClass={onOpenClass} />);
    const canvas = screen.getByRole("group", { name: en.models.graph.title });
    expect(within(canvas).getByRole("img", { name: "Enum SchoolLevel: primary, secondary" })).not.toHaveAttribute("tabindex");
    expect(within(canvas).getAllByRole("button").map((box) => box.getAttribute("aria-label"))).toEqual([
      opening("School"),
      opening("Person"),
    ]);
    expect(canvas).toHaveTextContent("from people");
    screen.getByRole("button", { name: opening("Person") }).focus();
    await userEvent.keyboard("{Enter}");
    expect(onOpenClass).toHaveBeenCalledExactlyOnceWith("Person");
  });

  it("says there is nothing to draw for a model with enums and no class", () => {
    renderPart(<LinkmlGraphView source={"enums:\n  E:\n    permissible_values:\n      a: {}\n"} />);
    expect(screen.getByRole("status")).toHaveTextContent(en.models.graph.empty);
  });

  it("opens the structure view on the class a click chose", async () => {
    const TWO = "name: two\nclasses:\n  Other:\n    slots: []\n  School:\n    slots: []\n";
    renderPart(<LinkmlEditor source={TWO} onChange={() => undefined} />);
    await userEvent.click(screen.getByRole("tab", { name: en.models.view.graph }));
    await userEvent.click(screen.getByRole("button", { name: opening("School") }));
    expect(screen.getByRole("button", { name: "School" })).toHaveAttribute("aria-current", "true");
  });

  it("draws the imported classes on the model page, read from the project's other model", async () => {
    const manifest = (name: string, linkml: string) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name, namespace: "helsinki" },
      spec: { contextSpaceRef: name, linkml, classes: [] },
    });
    await renderRoute({
      path: "/projects/helsinki/models/schools",
      answer: (path) =>
        path === "/api/v1/projects/helsinki/datamodels"
          ? jsonResponse(list([manifest("schools", SCHOOL), manifest("people", PEOPLE)]))
          : undefined,
    });
    expect(await screen.findByRole("button", { name: opening("Person") })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: en.models.graph.title })).toHaveTextContent("from people");
  });
});

describe("an empty model", () => {
  const EMPTY = "id: https://hel.fi/models/new\nname: new\nclasses: {}\nslots: {}\nenums: {}\n";

  // One "Add type" (owner 2026-11-01, T-6344): its dialog brings types from a Smart Data Model or a
  // spreadsheet, or starts an empty one by name; the three-step hints block left the page.
  it("adds a type from a Smart Data Model, a spreadsheet or a name, all from Add type", async () => {
    const onImport = vi.fn();
    renderPart(<LinkmlVisualEditor source={EMPTY} onChange={() => undefined} onImport={onImport} />);
    await userEvent.click(screen.getByRole("button", { name: en.models.addClass }));
    let dialog = screen.getByRole("dialog", { name: en.models.addClass });
    await userEvent.click(within(dialog).getByRole("button", { name: new RegExp(en.models.addFrom.sdm) }));
    expect(onImport).toHaveBeenLastCalledWith("sdm");
    expect(screen.queryByRole("dialog")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: en.models.addClass }));
    dialog = screen.getByRole("dialog", { name: en.models.addClass });
    await userEvent.click(within(dialog).getByRole("button", { name: new RegExp(en.models.addFrom.file) }));
    expect(onImport).toHaveBeenLastCalledWith("file");
    await userEvent.click(screen.getByRole("button", { name: en.models.addClass }));
    dialog = screen.getByRole("dialog", { name: en.models.addClass });
    expect(within(dialog).getByRole("textbox", { name: en.models.newClass })).toHaveAccessibleDescription(en.models.addClassNameHint);
  });

  it("offers no import where the page has none, only the name", async () => {
    renderPart(<LinkmlVisualEditor source={EMPTY} onChange={() => undefined} />);
    await userEvent.click(screen.getByRole("button", { name: en.models.addClass }));
    const dialog = screen.getByRole("dialog", { name: en.models.addClass });
    expect(within(dialog).queryByRole("button", { name: new RegExp(en.models.addFrom.sdm) })).toBeNull();
    expect(within(dialog).getByRole("textbox", { name: en.models.newClass })).toBeInTheDocument();
  });
});
