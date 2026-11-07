/**
 * T-5299 (EP-125): the Endpoint form greys out a surface the space's model cannot serve, from
 * GET …/spaces/{space}/surface-fit. A surface the Endpoint already lists stays free, so a person
 * can uncheck it; the journey with the page is e2e/surface_fit.spec.ts.
 */
import { describe, expect, it } from "vitest";
import { disabledRepresentations, surfaceGaps, type SurfaceFit } from "../src/api/surfaceFit";
import { ALWAYS_ON_REPRESENTATIONS } from "../src/schemas/kinds";

const GAP = "`sta` needs a type with a numeric attribute that declares its unit";
const FIT: SurfaceFit = {
  dataModel: "air",
  surfaces: [
    { representation: "sta", gap: GAP },
    { representation: "ical", gap: null },
  ],
};

describe("surface fit", () => {
  it("lists only the surfaces with a gap", () => {
    expect(surfaceGaps(FIT)).toEqual([{ representation: "sta", gap: GAP }]);
    expect(surfaceGaps(undefined)).toEqual([]);
  });

  it("greys out a gapped surface beside the always-on ones", () => {
    expect(disabledRepresentations(ALWAYS_ON_REPRESENTATIONS, FIT, ["ngsi-ld", "csv"])).toEqual(["ngsi-ld", "mcp", "sta"]);
  });

  it("leaves a gapped surface the Endpoint already lists free to uncheck", () => {
    expect(disabledRepresentations(ALWAYS_ON_REPRESENTATIONS, FIT, ["ngsi-ld", "sta"])).toEqual(["ngsi-ld", "mcp"]);
  });

  it("greys out nothing more while the answer has not come", () => {
    expect(disabledRepresentations(ALWAYS_ON_REPRESENTATIONS, undefined, undefined)).toEqual(["ngsi-ld", "mcp"]);
  });
});
