/**
 * T-4830: a model change is proposed with its diff and the version it becomes, the Form tab draws
 * every generated schema without an error, and no help text cites a task or a spec page.
 */
import { describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";
import { useShownErrors } from "../src/components/forms/touched";
import { classifyChanges, diffCounts, proposedVersion } from "../src/pages/models/breaking_detector";
import { formReady } from "../src/pages/models/formSchema";
import { parseModel } from "../src/pages/models/linkml";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";

const BEFORE = `id: https://hel.fi/models/air
name: air
classes:
  AirQualityObserved:
    slots: [pm10, no2]
slots:
  pm10: { range: float }
  no2: { range: float, description: Nitrogen dioxide. }
`;

describe("the diff and the version a proposal makes", () => {
  it("counts what was added, changed and removed, and bumps by the worst of it", () => {
    const after = BEFORE.replace("slots: [pm10, no2]", "slots: [pm10, no2, o3]").replace(
      "no2: { range: float, description: Nitrogen dioxide. }",
      "no2: { range: float, description: NO2 in µg/m³. }\n  o3: { range: float }",
    );
    const changes = classifyChanges(parseModel(BEFORE), parseModel(after));
    expect(diffCounts(changes)).toEqual({ added: 1, changed: 1, removed: 0 });
    expect(proposedVersion("1.0.0", changes, true)).toBe("1.1.0");

    const removed = classifyChanges(parseModel(BEFORE), parseModel(BEFORE.replace("slots: [pm10, no2]", "slots: [pm10]")));
    expect(diffCounts(removed).removed).toBeGreaterThan(0);
    expect(proposedVersion("1.4.2", removed, true)).toBe("2.0.0");
  });

  it("keeps the version of an unedited model and bumps the patch of an edit no field sees", () => {
    expect(proposedVersion("1.0.0", [], false)).toBe("1.0.0");
    expect(proposedVersion("1.0.0", [], true)).toBe("1.0.1");
  });
});

describe("the Form tab's schema", () => {
  const label = (type: string) => ({ number: "Number", string: "Text" })[type] ?? type;
  const generated = {
    type: "object",
    properties: {
      location: {
        type: ["object", "null"],
        "x-ngsi-ld-kind": "GeoProperty",
        properties: { type: { type: "string", enum: ["Point", "Polygon"] }, coordinates: { type: "array" } },
      },
      transportMode: { anyOf: [{ $ref: "#/definitions/TransportMode" }, { type: "null" }], description: "What serves the stop." },
      currentValue: { $ref: "#/definitions/Any", anyOf: [{ type: "number" }, { type: "string" }] },
    },
    definitions: {
      TransportMode: { type: "string", enum: ["bus", "tram"] },
      Address: { type: "object", properties: { box: { type: "array" } } },
    },
  };
  const example = { location: { type: "Polygon", coordinates: [[[24.9, 60.1], [25, 60.2], [24.9, 60.1]]] }, currentValue: 3 };
  const { schema, data } = formReady(generated, example, label, "Shown as JSON.");
  const props = schema.properties as Record<string, Record<string, unknown>>;

  it("leaves no array without items, anywhere, definitions included", () => {
    const bare = JSON.stringify(schema).match(/"type":"array"(?![^{}]*"items")/g);
    expect(bare).toBeNull();
    const coordinates = (props.location.properties as Record<string, Record<string, unknown>>).coordinates;
    expect(coordinates).toEqual({ type: "string", description: "Shown as JSON." });
    expect((data as { location: { coordinates: string } }).location.coordinates).toBe("[[[24.9,60.1],[25,60.2],[24.9,60.1]]]");
  });

  it("draws an optional value as that value, and names each option of a choice", () => {
    expect(props.transportMode).toEqual({ $ref: "#/definitions/TransportMode", description: "What serves the stop." });
    expect(props.currentValue).toEqual({ anyOf: [{ type: "number", title: "Number" }, { type: "string", title: "Text" }] });
    expect((data as { currentValue: number }).currentValue).toBe(3);
  });
});

describe("help texts", () => {
  // A task id or a spec page means nothing to the person reading the form.
  const internal = /\bT-\d{4}\b|\b(?:Development|Architecture|Requirements|Deployment|Testing)\/\d+\b/;
  const strings = (node: unknown, where = ""): [string, string][] =>
    typeof node === "string"
      ? [[where, node]]
      : node && typeof node === "object"
        ? Object.entries(node).flatMap(([key, value]) => strings(value, `${where}.${key}`))
        : [];

  it.each([
    ["cs", cs],
    ["de", de],
    ["en", en],
    ["sk", sk],
  ])("%s cites no task id or spec page", (_locale, messages) => {
    expect(strings(messages).filter(([, text]) => internal.test(text))).toEqual([]);
  });

  it("the check sees one", () => {
    expect(internal.test("a zero reads as a measurement (Development/10 §5)")).toBe(true);
    expect(internal.test("never shows English prose (T-4727)")).toBe(true);
  });
});

describe("a field's shown errors", () => {
  it("an empty list is no error, so an anyOf chooser is not marked invalid", () => {
    expect(renderHook(() => useShownErrors("root_currentValue", [])).result.current).toBeUndefined();
    expect(renderHook(() => useShownErrors("root_x", ["is required", "is required"])).result.current).toEqual(["is required"]);
  });
});
