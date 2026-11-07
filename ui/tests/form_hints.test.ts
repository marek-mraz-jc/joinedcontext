/**
 * UI-15, UI-16 (T-2137, CC-24): the pickers a schema asks for, read out of `x-jc-widget`.
 *
 * A JSON Schema can say a value is a string; only the Portal knows that the strings worth
 * offering are the spaces this project holds. `pickers` is the translation, and the two things
 * it must get right are both safety rather than convenience: a widget name the Portal does not
 * register is ignored rather than rendered, and a schema cannot talk its way into another
 * project by carrying its own `x-jc-options`.
 */
import { describe, expect, it } from "vitest";
import { pickers } from "../src/components/forms/hints";
import type { JsonSchema } from "../src/components/forms/types";

const schema = (properties: Record<string, unknown>): JsonSchema =>
  ({ type: "object", properties }) as unknown as JsonSchema;

describe("the pickers a schema asks for", () => {
  it("has nothing to say about a schema with no properties", () => {
    expect(pickers(undefined)).toEqual({});
    expect(pickers({ type: "string" } as unknown as JsonSchema)).toEqual({});
    expect(pickers(schema({}))).toEqual({});
  });

  it("names the widget a property asks for", () => {
    expect(pickers(schema({ space: { type: "string", "x-jc-widget": "resourcePicker" } }))).toEqual({
      space: { "ui:widget": "resourcePicker", "ui:options": {} },
    });
  });

  it("writes a prose parameter in a multi-line box, with no options (T-2757)", () => {
    expect(
      pickers(schema({ prompt: { type: "string", "x-jc-widget": "textarea", "x-jc-options": { rows: 99 } } }), {
        project: "helsinki",
      }),
    ).toEqual({ prompt: { "ui:widget": "textarea" } });
  });

  // EP-02, CC-28 (T-1571): the Portal mints an Endpoint's slug when the flow starts.
  it("hides a slug the Portal mints, so nobody is asked to type one", () => {
    expect(
      pickers(schema({ endpointSlug: { type: "string", "x-jc-widget": "endpointSlug" } }), { project: "helsinki" }),
    ).toEqual({ endpointSlug: { "ui:widget": "hidden" } });
  });

  it("leaves a property that asks for no widget alone", () => {
    // Everything else about the form still comes from the schema: a property with no hint is
    // rendered by RJSF's own input, and an empty entry would override that with nothing.
    expect(pickers(schema({ title: { type: "string" } }))).toEqual({});
  });

  it("ignores a widget the Portal does not register", () => {
    // A parameter with its default input is a better answer than a form that throws, and a
    // schema naming `<script>` or a component that does not exist must not reach the renderer.
    for (const widget of ["shellWidget", "", "constructor", "<script>", 7, null]) {
      expect(pickers(schema({ space: { type: "string", "x-jc-widget": widget } })), String(widget)).toEqual(
        {},
      );
    }
  });

  it("merges the page's options under the schema's own", () => {
    const ui = pickers(
      schema({ space: { type: "string", "x-jc-widget": "resourcePicker", "x-jc-options": { kind: "ContextSpace" } } }),
      { project: "banskabystrica" },
    );
    expect(ui.space).toEqual({
      "ui:widget": "resourcePicker",
      "ui:options": { project: "banskabystrica", kind: "ContextSpace" },
    });
  });

  it("lets a schema override a shared option, because the page is what renders it", () => {
    // The page passes what it knows; a schema that names `project` replaces it in its own
    // options and nowhere else. The page's own request is still made with the page's project,
    // which is what keeps this a rendering hint rather than a way into another project.
    const ui = pickers(
      schema({ space: { "x-jc-widget": "entityPicker", "x-jc-options": { project: "helsinki" } } }),
      { project: "banskabystrica" },
    );
    expect((ui.space as Record<string, Record<string, unknown>>)["ui:options"].project).toBe("helsinki");
    expect(pickers(schema({ space: { "x-jc-widget": "entityPicker" } }), { project: "banskabystrica" }).space).toEqual({
      "ui:widget": "entityPicker",
      "ui:options": { project: "banskabystrica" },
    });
  });

  it("ignores options that are not options", () => {
    for (const options of ["everything", 3, ["a"], null]) {
      const ui = pickers(schema({ space: { "x-jc-widget": "secretRef", "x-jc-options": options } }), {
        project: "bb",
      });
      expect((ui.space as Record<string, unknown>)["ui:options"], String(options)).toEqual({ project: "bb" });
    }
  });

  it("arranges the properties of an object parameter under its own name", () => {
    const ui = pickers(
      schema({
        target: {
          type: "object",
          properties: { space: { "x-jc-widget": "resourcePicker" }, note: { type: "string" } },
        },
      }),
      { project: "bb" },
    );
    expect(ui.target).toEqual({ space: { "ui:widget": "resourcePicker", "ui:options": { project: "bb" } } });
  });

  it("carries a nested picker without inventing an entry for its parent", () => {
    const ui = pickers(
      schema({ outer: { type: "object", properties: { inner: { type: "object", properties: {} } } } }),
    );
    expect(ui).toEqual({});
  });
});
