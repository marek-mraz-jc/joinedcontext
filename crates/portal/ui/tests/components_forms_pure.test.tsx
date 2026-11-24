/**
 * The small pure helpers of the forms folder: what each answers for the inputs the other tests
 * never send (T-7585).
 */
import { describe, expect, it } from "vitest";
import type { RJSFSchema, RJSFValidationError } from "@rjsf/utils";
import { errorMessageKey } from "../src/components/forms/errorMessages";
import { arrange, paths } from "../src/components/forms/uischema";
import validator from "../src/components/forms/validator";
import { toggled } from "../src/components/forms/widgets/attributeCapabilities";
import { presetOfRules } from "../src/components/forms/widgets/accessPresets";
import { withPreset, withSource, withType } from "../src/components/forms/widgets/icalMapping";
import { queryOf } from "../src/components/forms/widgets/representationOutputs";

describe("errorMessageKey on a pattern error it cannot read", () => {
  const error = { name: "pattern", schemaPath: "#/properties/name/pattern" } as RJSFValidationError;

  it("falls back to the generic pattern message without a schema", () => {
    expect(errorMessageKey(error)).toBe("form.pattern");
  });

  it("falls back when the schema path leads through a value that is not an object", () => {
    const schema = { type: "object", properties: { name: { pattern: "^x$" } } } as RJSFSchema;
    const through = { name: "pattern", schemaPath: "#/properties/name/pattern/deeper" } as RJSFValidationError;
    expect(errorMessageKey(through, schema)).toBe("form.pattern");
  });
});

describe("the validator", () => {
  it("answers a validationError, not a throw, for data that cannot be put on the wire", () => {
    const circular: Record<string, unknown> = {};
    circular.self = circular;
    const result = validator.rawValidation({ type: "object" } as RJSFSchema, circular);
    expect(result.errors).toBeUndefined();
    expect(result.validationError).toBeInstanceOf(Error);
  });
});

describe("uischema", () => {
  it("refuses a guide that leaves the documentation site and says so", () => {
    const arranged = arrange({ spec: { for: "Endpoint", guide: "https://elsewhere.example/page" } } as never);
    expect(arranged.guide).toBeUndefined();
    expect(arranged.problems).toEqual(['guide "https://elsewhere.example/page" is not a path inside the documentation site']);
  });

  it("refuses an empty guide path as well", () => {
    expect(arrange({ spec: { for: "Endpoint", guide: "  /  " } } as never).problems).toHaveLength(1);
  });

  it("skips a property whose definition is not an object when listing paths", () => {
    expect(paths({ properties: { a: { type: "string" }, b: null, c: true } })).toEqual(["a"]);
  });
});

describe("attribute capabilities toggled", () => {
  it("removes a value that was on, keeping the vocabulary's order", () => {
    expect(toggled(["a", "b", "c"], ["c", "a"], "c", false)).toEqual(["a"]);
    expect(toggled(["a", "b", "c"], undefined, "b", true)).toEqual(["b"]);
  });
});

describe("presetOfRules", () => {
  it("names no preset for a rule with verbs that are neither propose nor approve+propose", () => {
    expect(presetOfRules([{ kinds: ["Entity"], verbs: ["read"] }, { kinds: ["Entity"], verbs: ["approve"] }])).toBeNull();
  });
});

describe("ical mapping edits", () => {
  it("withSource clears a field when its attribute is emptied", () => {
    const next = withSource({ preset: "sdm-event", mapping: { SUMMARY: "title" } }, "SUMMARY", "  ");
    expect(next?.mapping?.SUMMARY).toBeUndefined();
  });

  it("withPreset own writes the base preset's required fields out", () => {
    const next = withPreset({ preset: "schema-org" }, "");
    expect(next?.preset).toBeUndefined();
    expect(next?.mapping?.SUMMARY).toBe("name");
  });

  it("withType trims the type and drops an empty one", () => {
    expect(withType({ preset: "sdm-event" }, "  Meeting ")?.type).toBe("Meeting");
    expect(withType({ preset: "sdm-event", type: "Meeting" }, " ")).toBeUndefined();
  });
});

describe("queryOf", () => {
  it("writes an older filter as a query string, escaping the characters the server decodes", () => {
    expect(queryOf({ filter: { q: "a&b+c#d%e", georel: "near" } })).toBe("q=a%26b%2Bc%23d%25e&georel=near");
  });
});
