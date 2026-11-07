import { describe, expect, it } from "vitest";
import type { JcEndpoint } from "@joinedcontext/sdk";
import { sourcesOf } from "./endpoints";

const endpoint = (name: string, types: string[]): JcEndpoint => ({ name, slug: name, space: "demo", types });

describe("sourcesOf", () => {
  it("gives an application of one endpoint a page per type, sorted, naming no endpoint", () => {
    expect(sourcesOf(["Station", "Note"], {})).toEqual([
      { id: "Note", type: "Note", shared: false },
      { id: "Station", type: "Station", shared: false },
    ]);
    expect(sourcesOf(["Note"], { endpoints: [endpoint("a", ["Note"])] })).toEqual([{ id: "Note", type: "Note", shared: false }]);
    expect(sourcesOf([], { endpoints: [endpoint("a", []), endpoint("b", [])] })).toEqual([]);
  });

  it("reads each type through the endpoint that serves it, the primary for a type none names", () => {
    const config = { endpoints: [endpoint("a", ["Note"]), endpoint("b", ["Station"])] };
    expect(sourcesOf(["Station", "Note", "Sensor"], config)).toEqual([
      { id: "Note", type: "Note", endpoint: "a", shared: false },
      { id: "Sensor", type: "Sensor", endpoint: "a", shared: false },
      { id: "Station", type: "Station", endpoint: "b", shared: false },
    ]);
  });

  it("gives a type several endpoints serve one source per endpoint, with an id of its own", () => {
    const config = { endpoints: [endpoint("north", ["Parking"]), endpoint("south", ["Parking", "Note"])] };
    expect(sourcesOf(["Parking", "Note"], config)).toEqual([
      { id: "Note", type: "Note", endpoint: "south", shared: false },
      { id: "Parking@north", type: "Parking", endpoint: "north", shared: true },
      { id: "Parking@south", type: "Parking", endpoint: "south", shared: true },
    ]);
  });
});
