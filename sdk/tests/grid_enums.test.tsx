/**
 * An enum is picked, never typed (T-2706, UI-86): one reader resolves the shapes Model Tools
 * writes a LinkML enum in, the grid's edit cell and filter row offer exactly its values, and a
 * stored value outside the enum is marked and left alone.
 */
import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { enumOptions } from "../src/enums";
import { fieldOf } from "../src/write";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import { queryFromFilters } from "../src/grid/filters";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";

const defs = {
  AlertCategory: { type: "string", enum: ["traffic", "weather", "health"], title: "AlertCategory" },
  Severity: {
    oneOf: [
      { const: "low", title: { en: "Low", sk: "Nízka" }, description: "Nothing to do" },
      { const: "high", title: { en: "High", sk: "Vysoká" } },
    ],
  },
  Loop: { $ref: "#/definitions/Loop" },
};

describe("the enum reader", () => {
  it("reads an enum written directly on the property", () => {
    expect(enumOptions({ type: "string", enum: ["a", "b"] })).toEqual([
      { value: "a", title: undefined, description: undefined },
      { value: "b", title: undefined, description: undefined },
    ]);
  });

  it("follows a $ref into definitions and $defs", () => {
    const values = (ref: string) => enumOptions({ $ref: ref }, defs)?.map((option) => option.value);
    expect(values("#/definitions/AlertCategory")).toEqual(["traffic", "weather", "health"]);
    expect(values("#/$defs/AlertCategory")).toEqual(["traffic", "weather", "health"]);
  });

  it("reads an optional slot: an anyOf of the enum and null", () => {
    const property = { anyOf: [{ $ref: "#/definitions/AlertCategory" }, { type: "null" }] };
    expect(enumOptions(property, defs)?.map((option) => option.value)).toEqual(["traffic", "weather", "health"]);
  });

  it("reads an allOf that wraps the reference", () => {
    const property = { allOf: [{ $ref: "#/definitions/AlertCategory" }], description: "The domain" };
    expect(enumOptions(property, defs)).toHaveLength(3);
  });

  it("gives each value its title in the person's language", () => {
    const sk = enumOptions({ $ref: "#/definitions/Severity" }, defs, "sk-SK");
    expect(sk).toEqual([
      { value: "low", title: "Nízka", description: "Nothing to do" },
      { value: "high", title: "Vysoká", description: undefined },
    ]);
    // A language the model does not have falls back to English.
    expect(enumOptions({ $ref: "#/definitions/Severity" }, defs, "fi")?.[0].title).toBe("Low");
  });

  it("reads titles given beside a plain enum", () => {
    const property = { enum: ["a"], "x-enum-titles": { a: { en: "Alpha" } } };
    expect(enumOptions(property, {}, "en")).toEqual([{ value: "a", title: "Alpha", description: undefined }]);
  });

  it("answers null for what is not an enum: text, a missing or cyclic reference, two enums", () => {
    expect(enumOptions({ type: "string" })).toBeNull();
    expect(enumOptions({ $ref: "#/definitions/Nothing" }, defs)).toBeNull();
    expect(enumOptions({ $ref: "#/definitions/Loop" }, defs)).toBeNull();
    expect(enumOptions({ $ref: "https://example.org/schema#/definitions/AlertCategory" }, defs)).toBeNull();
    expect(enumOptions({ anyOf: [{ $ref: "#/definitions/AlertCategory" }, { $ref: "#/definitions/Severity" }] }, defs)).toBeNull();
    expect(enumOptions({ enum: [] })).toBeNull();
    expect(enumOptions(null)).toBeNull();
  });

  it("makes a form field a select over the resolved values", () => {
    const field = fieldOf("category", { properties: { category: { $ref: "#/definitions/AlertCategory" } } }, "text", defs);
    expect(field.input).toBe("select");
    expect(field.options?.map((option) => option.value)).toEqual(["traffic", "weather", "health"]);
  });
});

describe("the filter row", () => {
  it("builds NGSI-LD's value list from the picked values, each quoted", () => {
    const columns = [{ key: "category", attr: "category", kind: "enum" as const }];
    expect(queryFromFilters(columns, { category: { op: "anyOf", value: "", values: ["traffic", "we\"ird"] } })).toEqual({
      q: 'category=="traffic","we\\"ird"',
      idPattern: undefined,
    });
    // Nothing picked asks for nothing.
    expect(queryFromFilters(columns, { category: { op: "anyOf", value: "", values: [] } }).q).toBeUndefined();
  });
});

const alerts: Record<string, unknown>[] = [
  { id: "urn:ngsi-ld:Alert:hel:helsinki:1", type: "Alert", category: { type: "Property", value: "traffic" } },
  { id: "urn:ngsi-ld:Alert:hel:helsinki:2", type: "Alert", category: { type: "Property", value: "roadworks" } },
];

const options = [
  { value: "traffic", title: "Traffic" },
  { value: "weather", title: "Weather" },
];

function grid(mode: "view" | "edit") {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "alerts" },
    type: "Alert",
    columns: [{ attr: "category", label: "Category" }],
    ...(mode === "edit" ? { mode, editableAttrs: ["category"] } : {}),
    pageSize: 10,
  });
  const inner = fixtureSource(alerts);
  const asked: (string | undefined)[] = [];
  const written: { id: string; attrs: Record<string, unknown> }[] = [];
  const source = {
    ...inner,
    query: async (...args: Parameters<typeof inner.query>) => {
      asked.push(args[0].q);
      return inner.query(...args);
    },
    patch: async (id: string, attrs: Record<string, unknown>) => {
      written.push({ id, attrs });
    },
  };
  render(<EntityGrid config={parsed.config!} source={source} enums={{ category: options }} />);
  return { asked, written };
}

describe("an enum column in the grid", () => {
  it("hints its filter with the allowed values, by title (T-4949, UI-16)", async () => {
    grid("view");
    const filter = await screen.findByLabelText("Filter: Category");
    expect(filter).toHaveAccessibleDescription(expect.stringContaining(DEFAULT_LABELS.filterHintEnum));
    expect(filter).toHaveAccessibleDescription(expect.stringContaining("Traffic"));
  });

  it("reads a stored value as its title, and one outside the enum as written (T-4200)", async () => {
    grid("view");
    // The filter row's options say the titles too; the cells are what is asked about here.
    const cell = { ignore: "option, script, style" };
    expect(await screen.findByText("Traffic", cell)).toBeInTheDocument();
    expect(screen.queryByText("traffic", cell)).toBeNull();
    // A value outside the list says so (G-R2GRID2-019).
    expect(screen.getByText(`roadworks (${DEFAULT_LABELS.notInList})`, cell)).toBeInTheDocument();
  });

  it("edits by picking exactly the permissible values, showing titles and storing the value", async () => {
    const { written } = grid("edit");
    const cells = await screen.findAllByLabelText(`${DEFAULT_LABELS.edit} Category`);
    const first = cells[0] as HTMLSelectElement;
    expect(first.tagName).toBe("SELECT");
    expect(Array.from(first.options, (option) => [option.value, option.text])).toEqual([
      ["traffic", "Traffic"],
      ["weather", "Weather"],
    ]);
    fireEvent.change(first, { target: { value: "weather" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));
    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0].attrs).toEqual({ category: { type: "Property", value: "weather" } });
  });

  it("marks a stored value outside the enum and never changes it by itself", async () => {
    grid("edit");
    const cells = await screen.findAllByLabelText(`${DEFAULT_LABELS.edit} Category`);
    const second = cells[1] as HTMLSelectElement;
    expect(second.value).toBe("roadworks");
    expect(second.getAttribute("aria-invalid")).toBe("true");
    expect(second.options[0].text).toBe(`roadworks (${DEFAULT_LABELS.notInList})`);
    // Showing it is not a change.
    expect(screen.queryByText(new RegExp(DEFAULT_LABELS.pending))).toBeNull();
    // The one inside the enum is not marked.
    expect((cells[0] as HTMLSelectElement).getAttribute("aria-invalid")).toBeNull();
  });

  it("leaves the arrow keys to the picker instead of moving the active cell", async () => {
    grid("edit");
    const cells = await screen.findAllByLabelText(`${DEFAULT_LABELS.edit} Category`);
    const table = screen.getByRole("grid");
    fireEvent.keyDown(table, { key: "ArrowDown" });
    const active = () => document.querySelector("[aria-selected='true'], [data-active='true']")?.textContent ?? null;
    const before = active();
    const event = new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true });
    cells[0].dispatchEvent(event);
    // Not prevented: the select changes its value as the browser does, and the grid stays put.
    expect(event.defaultPrevented).toBe(false);
    expect(active()).toBe(before);
    // Picking by keyboard is the select's own change event.
    fireEvent.change(cells[0], { target: { value: "weather" } });
    expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument();
  });

  it("picks one value in the filter row, several from the header's list, and asks for any of them", async () => {
    const { asked } = grid("view");
    await screen.findByText(`roadworks (${DEFAULT_LABELS.notInList})`);
    fireEvent.change(screen.getByLabelText(`${DEFAULT_LABELS.filter}: Category`), { target: { value: "anyOf" } });
    const picker = screen.getByLabelText(`${DEFAULT_LABELS.value}: Category`) as HTMLSelectElement;
    expect(picker.multiple).toBe(false);
    expect(Array.from(picker.options, (option) => option.text)).toEqual([DEFAULT_LABELS.noFilter, "Traffic", "Weather"]);
    fireEvent.change(picker, { target: { value: "weather" } });
    await waitFor(() => expect(asked).toContain('category=="weather"'));
  });
});

describe("enumsOf", () => {
  it("lists every enum attribute of a type and nothing else", async () => {
    const { enumsOf } = await import("../src/enums");
    const type = {
      properties: {
        category: { $ref: "#/definitions/AlertCategory" },
        name: { type: "string" },
        severity: { anyOf: [{ $ref: "#/definitions/Severity" }, { type: "null" }] },
      },
    };
    expect(Object.keys(enumsOf(type, defs))).toEqual(["category", "severity"]);
    expect(enumsOf(undefined)).toEqual({});
  });
});
