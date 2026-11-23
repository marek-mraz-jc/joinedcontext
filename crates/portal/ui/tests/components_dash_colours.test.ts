// @vitest-environment node
import { describe, expect, it } from "vitest";
import { themeColour } from "../src/components/dashboards/mapColours";

describe("themeColour without a document", () => {
  it("answers the fallback where the code runs outside a browser", () => {
    expect(typeof document).toBe("undefined");
    expect(themeColour("--portal-surface-subtle", "#eef1f4")).toBe("#eef1f4");
  });
});
