import { describe, expect, it } from "vitest";
import { PORTAL_WIDGET_NAMES } from "../src/components/forms/widgetNames";
import { portalWidgets } from "../src/components/forms/widgets";

describe("the Portal's widget names", () => {
  it("are the keys of the widgets themselves, none missing and none extra", () => {
    expect([...PORTAL_WIDGET_NAMES].sort()).toEqual(Object.keys(portalWidgets).sort());
  });
});
