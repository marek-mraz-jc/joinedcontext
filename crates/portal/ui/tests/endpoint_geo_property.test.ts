// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/endpoints/EndpointPage.tsx.
/**
 * T-4947: the area filter's geometry attribute is a choice of the GeoProperties the projected
 * types declare, NGSI-LD's default `location` first, never a typed name.
 */
import { describe, expect, it } from "vitest";
import { geoPropertiesOf } from "../src/pages/endpoints/EndpointPage";

const SOURCE = `
id: https://example.org/parking
name: parking
classes:
  ParkingSpot:
    slots: [location, entrance, status]
slots:
  location:
    annotations:
      ngsi_ld_kind: GeoProperty
  entrance:
    annotations:
      ngsi_ld_kind: GeoProperty
  status:
    range: string
`;

describe("the area filter's geometry attribute (T-4947)", () => {
  it("offers the GeoProperties of the projected types, location first and once", () => {
    expect(geoPropertiesOf(SOURCE, ["ParkingSpot"])).toEqual(["location", "entrance"]);
  });

  it("offers location alone when no model is read or no type declares one", () => {
    expect(geoPropertiesOf(undefined, ["ParkingSpot"])).toEqual(["location"]);
    expect(geoPropertiesOf(SOURCE, ["Road"])).toEqual(["location"]);
  });
});
