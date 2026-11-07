/**
 * What an application is called on screen (AP-69): its title, else its name read as words with
 * the endpoint's title after it; never the id.
 */
import { describe, expect, it } from "vitest";
import { appDisplayName, humanizeName } from "../src/pages/apps/appTitle";

describe("the application's display name", () => {
  it("reads a name as words", () => {
    expect(humanizeName("map-visualization")).toBe("Map visualization");
    expect(humanizeName("kpi_board.v2")).toBe("Kpi board v2");
    expect(humanizeName("")).toBe("");
  });

  it("prefers the title, then the name as words with the endpoint's title", () => {
    expect(
      appDisplayName({ title: "Helsinki Traffic Alerts Map", appName: "map-visualization", endpointTitle: "Helsinki traffic alerts" }),
    ).toBe("Helsinki Traffic Alerts Map");
    expect(appDisplayName({ title: "  ", appName: "map-visualization", endpointTitle: "Helsinki traffic alerts" })).toBe(
      "Map visualization · Helsinki traffic alerts",
    );
    expect(appDisplayName({ appName: "map-visualization" })).toBe("Map visualization");
    expect(appDisplayName({ appName: "", endpointTitle: "Helsinki traffic alerts" })).toBe("Helsinki traffic alerts");
  });
});
