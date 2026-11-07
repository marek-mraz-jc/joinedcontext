import { describe, expect, it } from "vitest";
import { REPRESENTATION_PATHS } from "../src/components/endpoints/links";

/**
 * GW33 (CIM 009 5.7.2.4): the NGSI-LD read surface answers as the specification says, so a
 * query naming no selector is `400 BadRequestData` and a grant never rescues it. Every link
 * and sample read the Portal sends therefore names a type, or asks for the type list, which
 * needs no selector. T-0780.
 */
describe("the Portal's own NGSI-LD reads", () => {
  it("never links to an entity query without a selector", () => {
    const path = REPRESENTATION_PATHS["ngsi-ld"];
    expect(path).toBeDefined();
    const entityQuery = path.startsWith("/ngsi-ld/v1/entities");
    expect(
      !entityQuery || /[?&](type|id|idPattern)=/.test(path),
      `the ngsi-ld link must name a type or ask for the type list, got ${path}`,
    ).toBe(true);
  });

  it("asks the endpoint which types it serves", () => {
    expect(REPRESENTATION_PATHS["ngsi-ld"]).toBe("/ngsi-ld/v1/types");
  });

  it("leaves the other representations alone", () => {
    expect(REPRESENTATION_PATHS.csv).toBe("/file.csv");
    // CSV is the one file (T-4600).
    expect(REPRESENTATION_PATHS.geojson).toBeUndefined();
    expect(REPRESENTATION_PATHS.mcp).toBe("/mcp");
  });
});
