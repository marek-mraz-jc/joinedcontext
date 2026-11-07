import { describe, expect, it } from "vitest";
import { fieldsOf, ownerQuery, problemsOf, STEPS, valuesOf } from "./steps";
import { SCHEMA } from "./fixtures";

const fields = fieldsOf(SCHEMA.ServiceRequest);
const of = (names: string[]) => names.map((name) => fields[name]);

describe("the steps", () => {
  it("takes each input from the schema the LinkML model generates", () => {
    expect(fields.category).toMatchObject({ input: "select", required: true, options: ["pothole", "streetlight", "graffiti", "litter", "other"].map((value) => ({ value })) });
    expect(fields.title).toMatchObject({ input: "text", required: true, pattern: ".{5,80}" });
    expect(fields.district).toMatchObject({ input: "select", required: false });
    expect(fields.mayContact).toMatchObject({ input: "checkbox", required: false });
    expect(STEPS.at(-1)?.fields).toEqual([]);
  });

  it("names every required answer that is missing, and every answer that breaks its rule", () => {
    expect(problemsOf(of(STEPS[0].fields), {})).toEqual({ category: "is required", title: "is required", description: "is required" });
    expect(problemsOf(of(STEPS[0].fields), { category: "volcano", title: "Hole", description: "   " })).toEqual({
      category: "must be one of pothole, streetlight, graffiti, litter, other",
      title: "does not match the expected format",
      description: "is required",
    });
    expect(problemsOf(of(STEPS[2].fields), {})).toEqual({});
    expect(problemsOf(of(STEPS[2].fields), { contactEmail: "aino at example" })).toEqual({ contactEmail: "does not match the expected format" });
    expect(problemsOf(of(STEPS[2].fields), { contactEmail: "aino@example.fi" })).toEqual({});
  });

  it("types the answers and leaves the empty ones out", () => {
    expect(valuesOf(of(["title", "district", "mayContact", "contactEmail"]), { title: "Pothole at the stop", district: "", mayContact: "true" })).toEqual({
      title: "Pothole at the stop",
      mayContact: true,
    });
  });

  it("only puts an id that looks like an account id into a query", () => {
    expect(ownerQuery("5b0e7c1a-2f4d-4c8e-9a61-3d2b7f0c9e14")).toBe('submittedBy=="5b0e7c1a-2f4d-4c8e-9a61-3d2b7f0c9e14"');
    expect(ownerQuery('x"||submittedBy!="x')).toBeNull();
    expect(ownerQuery("")).toBeNull();
    expect(ownerQuery("a".repeat(129))).toBeNull();
  });
});
