// T-7706: geometryOf reads a normalized Relationship as its object and finds no geometry in it.
import { describe, expect, it } from "vitest";
import { geometryOf } from "../src/pages/explore/exportView";

describe("geometryOf", () => {
  it("reads a relationship as its target and finds the geometry of another attribute", () => {
    const point = { type: "Point", coordinates: [24.9, 60.2] };
    expect(geometryOf({ id: "urn:x", refStreet: { type: "Relationship", object: "urn:ngsi-ld:Street:1" } })).toBeUndefined();
    expect(geometryOf({ refStreet: { type: "Relationship", object: "urn:ngsi-ld:Street:1" }, place: { type: "GeoProperty", value: point } })).toEqual(point);
  });
});
