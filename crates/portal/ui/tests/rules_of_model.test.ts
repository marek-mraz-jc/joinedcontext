/** The grid's check at the cell takes each slot's rule from the space's LinkML model (T-3097). */
import { describe, expect, it } from "vitest";
import { rulesOfModel } from "../src/components/entities/filters";

const STATION = `name: stations
classes:
  Station:
    attributes:
      name:
        range: string
        required: true
      capacity:
        range: integer
        minimum_value: 0
        maximum_value: 500
      load:
        range: float
      code:
        range: string
        pattern: "^[A-Z]{3}$"
      site:
        range: uri
      owner:
        range: Person
  Person:
    attributes:
      age:
        range: integer
`;

describe("rulesOfModel", () => {
  it("maps each slot's range, bounds, pattern and required onto the grid's rule", () => {
    expect(rulesOfModel(STATION, "Station")).toEqual({
      name: { kind: "string", required: true },
      capacity: { kind: "integer", minimum: 0, maximum: 500 },
      load: { kind: "number" },
      code: { kind: "string", pattern: "^[A-Z]{3}$" },
      site: { kind: "uri" },
    });
  });

  it("states nothing without a model, a type, or a class of that name", () => {
    expect(rulesOfModel(undefined, "Station")).toEqual({});
    expect(rulesOfModel(STATION, undefined)).toEqual({});
    expect(rulesOfModel(STATION, "Bridge")).toEqual({});
  });
});
