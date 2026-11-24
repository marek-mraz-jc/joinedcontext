// T-7706: fieldWidget.ts edges: a reference that points nowhere, a wrapper chain deeper than 16, enums that
// are not plain strings, a duration by format, and a manifest path through a value that is not an object.
import { describe, expect, it } from "vitest";
import { fieldWidget, specValue } from "../src/components/grid/fieldWidget";

const spec = (field: Record<string, unknown>) => ({ type: "object", properties: { spec: { type: "object", properties: { f: field } } } });

describe("fieldWidget edges", () => {
  it("reads a field whose $ref names no definition as the field itself", () => {
    expect(fieldWidget("K", spec({ $ref: "#/elsewhere/X", type: "string" }), "f")?.widget).toEqual({ kind: "text" });
    expect(fieldWidget("K", spec({ $ref: "#/definitions/Missing", type: "integer" }), "f")?.widget).toMatchObject({ kind: "number", integer: true });
  });

  it("stops unwrapping after sixteen wrappers and reads nothing from what is left", () => {
    let field: Record<string, unknown> = { type: "string" };
    for (let i = 0; i < 20; i += 1) field = { allOf: [field] };
    expect(fieldWidget("K", spec(field), "f")).toBeNull();
  });

  it("offers no dropdown for a oneOf that holds a non-schema, a branch with no value, or a non-text value", () => {
    expect(fieldWidget("K", spec({ type: "string", oneOf: ["x"] }), "f")?.widget).toEqual({ kind: "text" });
    expect(fieldWidget("K", spec({ type: "string", oneOf: [{ description: "no value" }] }), "f")?.widget).toEqual({ kind: "text" });
    expect(fieldWidget("K", spec({ type: "string", oneOf: [{ enum: ["a", 3] }] }), "f")?.widget).toEqual({ kind: "text" });
    expect(fieldWidget("K", spec({ oneOf: [{ const: "a", description: "A" }, { enum: ["b"] }] }), "f")?.widget).toEqual({
      kind: "enum",
      nullable: false,
      options: [{ value: "a", description: "A" }, { value: "b", description: undefined }],
    });
  });

  it("calls a string with the duration format a duration", () => {
    expect(fieldWidget("K", spec({ type: "string", format: "duration" }), "f")?.widget).toEqual({ kind: "duration" });
  });

  it("gives no value for a path that runs through a value that is not an object", () => {
    expect(specValue({ spec: { a: "text" } }, "a.b")).toBeUndefined();
    expect(specValue({}, "a")).toBeUndefined();
    expect(specValue({ spec: { a: { b: 2 } } }, "a.b")).toBe(2);
  });
});
