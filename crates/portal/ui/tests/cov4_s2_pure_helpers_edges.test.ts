// T-7706: small helpers with one branch no test took: where a refusal of the KPI check shows, the explorer's
// scopeQ in a snippet, removing a key from a proposal's field list, and the default colour of a panel.
import { describe, expect, it } from "vitest";
import { placeOf } from "../src/pages/pipelines/kpiEditor";
import { viewPath } from "../src/pages/explore/snippets";
import { applyFields } from "../src/pages/catalogue/publish";
import { colorOf } from "../src/components/panels/palette";

describe("placeOf", () => {
  it("puts a refusal of the KPI name, the schedule and the outputs at the field that holds it", () => {
    expect(placeOf("spec.steps.kpi.name")).toEqual({ at: "name" });
    expect(placeOf("metadata.name")).toEqual({ at: "name" });
    expect(placeOf("spec.schedule.every")).toEqual({ at: "schedule" });
    expect(placeOf("spec.outputs[0].space")).toEqual({ at: "target" });
    expect(placeOf("spec.unknown")).toEqual({ at: "general" });
  });
});

describe("viewPath", () => {
  it("carries a trimmed scope filter and drops a blank one", () => {
    expect(viewPath({ scopeQ: " /city/# " })).toContain("scopeQ=%2Fcity%2F%23");
    expect(viewPath({ scopeQ: "   " })).not.toContain("scopeQ");
  });
});

describe("applyFields", () => {
  it("removes a key of an object when the field has no new value", () => {
    expect(applyFields({ spec: { a: 1, b: 2 } }, [{ path: "spec.a" }])).toEqual({ spec: { b: 2 } });
  });
});

describe("colorOf", () => {
  it("is the text colour for a panel that names none", () => {
    expect(colorOf(undefined)).toBe(colorOf(""));
    expect(colorOf(undefined)).not.toBe("");
  });
});
