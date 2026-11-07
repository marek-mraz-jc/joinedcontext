import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { NETWORK_ROWS } from "./fixtures/network";
import { entitiesOf, pointKey, stopIdOf, toNetwork } from "./network";

describe("HSL's network as the module takes it", () => {
  it("keeps placed stops with their name and code, and lines with a number and two stops", () => {
    const stops = NETWORK_ROWS.filter((r) => r.type === "GtfsStop");
    const routes = NETWORK_ROWS.filter((r) => r.type === "PublicTransportRoute");
    const network = toNetwork(stops, routes);
    expect(network.stops[0]).toEqual({ id: "1020601", lon: 24.9414, lat: 60.171, name: "Rautatientori", code: "H0019" });
    expect(network.stops[3]).toEqual({ id: "1111602", lon: 24.9561, lat: 60.1755, name: "Kaisaniemenranta" });
    expect(network.routes).toEqual([
      { name: "M1", mode: "metro", stops: ["1020601", "1020602", "1111601"] },
      { name: "550", mode: "bus", stops: ["1020602", "1111602"] },
    ]);
  });

  it("drops a stop with no point and a line with no number or a single stop", () => {
    const network = toNetwork(
      [{ id: "urn:x:1", type: "GtfsStop", name: "Nowhere", location: { type: "LineString", coordinates: [] } } as Row, { id: "urn:x:2", type: "GtfsStop" } as Row],
      [
        { id: "urn:y:1", type: "PublicTransportRoute", routeShortName: "", stopSequence: "1, 2" } as Row,
        { id: "urn:y:2", type: "PublicTransportRoute", routeShortName: "7", stopSequence: "1" } as Row,
        { id: "urn:y:3", type: "PublicTransportRoute", routeShortName: "8", stopSequence: " 1 , ,2" } as Row,
      ],
    );
    expect(network.stops).toEqual([]);
    expect(network.routes).toEqual([{ name: "8", stops: ["1", "2"] }]);
  });

  it("reads a stop's GTFS id off the end of its entity id", () => {
    expect(stopIdOf("urn:ngsi-ld:GtfsStop:hel.fi:helsinki:1020601")).toBe("1020601");
  });

  it("reads a numbered name, refuses a point of text, and keys each placed stop's entity by its point", () => {
    const stops = [
      { id: "urn:x:1", type: "GtfsStop", name: 42, stopCode: "  ", location: { type: "Point", coordinates: [24.9, 60.1] } } as unknown as Row,
      { id: "urn:x:2", type: "GtfsStop", location: { type: "Point", coordinates: ["24.9", 60.1] } } as unknown as Row,
      { id: "urn:x:3", type: "GtfsStop", location: null } as unknown as Row,
    ];
    expect(toNetwork(stops, []).stops).toEqual([{ id: "1", lon: 24.9, lat: 60.1, name: "42" }]);
    expect(entitiesOf(stops)).toEqual(new Map([[pointKey({ lon: 24.9, lat: 60.1 }), "urn:x:1"]]));
  });
});
