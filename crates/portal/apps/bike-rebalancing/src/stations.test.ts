import { describe, expect, it } from "vitest";
import { DEFAULT_TOKENS } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { STATIONS } from "./fixtures/stations";
import { countOf, inService, levelColour, nameOf, newest, stationsOf } from "./stations";

describe("stations", () => {
  it("reads a count only when it is a whole number of bikes", () => {
    expect(countOf(4)).toBe(4);
    expect(countOf(0)).toBe(0);
    for (const broken of [undefined, null, -1, 2.5, "4", Number.NaN]) expect(countOf(broken)).toBeNull();
  });

  it("routes no station that is out of service, and keeps one with no status", () => {
    const [working, , , , , , , closed] = STATIONS;
    expect(inService(working)).toBe(true);
    expect(inService(closed)).toBe(false);
    expect(inService({ id: "x", type: "BikeHireDockingStation" } as Row)).toBe(true);
    const out = stationsOf([closed])[0];
    expect([out.bikes, out.free, out.capacity]).toEqual([null, null, null]);
    expect([out.lon, out.lat]).toEqual([24.9414, 60.171]);
  });

  it("keeps a missing count and a missing place as unknown", () => {
    const [viiskulma] = stationsOf([STATIONS[3]]);
    expect(viiskulma.bikes).toBeNull();
    expect(viiskulma.free).toBe(14);
    const [nowhere] = stationsOf([{ id: "urn:ngsi-ld:BikeHireDockingStation:x:y:9", type: "BikeHireDockingStation" } as Row]);
    expect([nowhere.lon, nowhere.lat, nowhere.name]).toEqual([null, null, "9"]);
    expect(stationsOf([])).toEqual([]);
  });

  it("names a station by its name, else its id", () => {
    expect(nameOf(STATIONS[0])).toBe("Kaivopuisto");
    expect(nameOf({ id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:042", type: "x" } as Row)).toBe("042");
  });

  it("colours each level from the design tokens", () => {
    expect(levelColour("empty", DEFAULT_TOKENS)).toBe(DEFAULT_TOKENS.color.danger);
    expect(levelColour("full", DEFAULT_TOKENS)).toBe(DEFAULT_TOKENS.color.accent);
    expect(levelColour("unknown", DEFAULT_TOKENS)).toBe(DEFAULT_TOKENS.color.muted);
  });

  it("finds when the counts were read, ignoring a broken date", () => {
    expect(newest(STATIONS)?.toISOString()).toBe("2030-10-20T05:55:00.000Z");
    expect(newest([{ id: "a", type: "x", dateModified: "soon" } as Row])).toBeNull();
    expect(newest([])).toBeNull();
  });
});
