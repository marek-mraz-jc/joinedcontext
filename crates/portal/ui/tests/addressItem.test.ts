import { describe, expect, it } from "vitest";
import { addressItem, crumbModel } from "../src/components/layout/Shell";

describe("addressItem", () => {
  it("names the item of a section by its address", () => {
    expect(addressItem("/projects/p/endpoints/air-quality")).toEqual({ itemName: "air-quality" });
    expect(addressItem("/projects/p/spaces/city/data")).toEqual({ itemName: "city" });
  });

  it("decodes a segment and keeps a broken escape as typed", () => {
    expect(addressItem("/projects/p/endpoints/a%20b")).toEqual({ itemName: "a b" });
    expect(addressItem("/projects/p/endpoints/a%zz")).toEqual({ itemName: "a%zz" });
  });

  it("names a static Context Spaces page in words, never as a space", () => {
    expect(addressItem("/projects/p/spaces/complete")).toEqual({ spacePage: "spaces.complete.title" });
    expect(addressItem("/projects/p/spaces/upload")).toEqual({ spacePage: "spaces.upload.title" });
    // Deeper, it is a space of that name; elsewhere, an item of that name.
    expect(addressItem("/projects/p/spaces/upload/data")).toEqual({ itemName: "upload" });
    expect(addressItem("/projects/p/endpoints/upload")).toEqual({ itemName: "upload" });
  });

  it("marks a create form, and names nothing on a section list or outside a project", () => {
    expect(addressItem("/projects/p/endpoints/new")).toEqual({ creating: true });
    expect(addressItem("/projects/p/endpoints")).toEqual({});
    expect(addressItem("/organization/people/x")).toEqual({});
  });
});

describe("crumbModel", () => {
  it("names a model only on the models pages", () => {
    expect(crumbModel(false, "air", undefined)).toBeUndefined();
    expect(crumbModel(true, undefined, undefined)).toBeUndefined();
  });

  it("takes the address item first, then the editor's model, and \"\" for a new one", () => {
    expect(crumbModel(true, "air", "other")).toBe("air");
    expect(crumbModel(true, undefined, "air")).toBe("air");
    expect(crumbModel(true, undefined, "")).toBe("");
  });
});
