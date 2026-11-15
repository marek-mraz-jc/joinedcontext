import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { EMPTY, filterOf, helsinkiMidnight, readView, startOf, toInput, writeView } from "./alerts";

const row = (fields: Record<string, unknown>): Row => ({ id: "urn:ngsi-ld:Alert:hel.fi:helsinki:X", type: "Alert", ...fields }) as Row;

describe("an alert as the analysis reads it", () => {
  it("starts at validFrom, else when it was issued, else never", () => {
    expect(startOf(row({ validFrom: "2030-10-07T05:00:00Z", dateIssued: "2030-10-01T00:00:00Z" }))).toBe(Date.parse("2030-10-07T05:00:00Z"));
    expect(startOf(row({ dateIssued: "2030-10-01T00:00:00Z" }))).toBe(Date.parse("2030-10-01T00:00:00Z"));
    expect(startOf(row({ validFrom: "not a date" }))).toBeNull();
    expect(startOf(row({}))).toBeNull();
  });

  it("hands over the geometry as it came and an empty kind when it has none", () => {
    const geometry = { type: "Point", coordinates: [24.9, 60.2] };
    expect(toInput(row({ location: geometry, subCategory: "roadWorks" }))).toEqual({ id: "urn:ngsi-ld:Alert:hel.fi:helsinki:X", geometry, time: null, subCategory: "roadWorks" });
    expect(toInput(row({}))).toMatchObject({ geometry: null, subCategory: "" });
  });
});

describe("the view in the address", () => {
  it("round-trips and keeps the language", () => {
    const view = { from: "2030-10-01", to: "2030-10-31", kinds: ["roadWorks", "carAccident"], weekday: 0, hour: 8 };
    const search = writeView("?lang=en", view);
    expect(search).toBe("?lang=en&from=2030-10-01&to=2030-10-31&kind=roadWorks%2CcarAccident&day=0&hour=8");
    expect(readView(search)).toEqual(view);
    expect(writeView("?lang=en", EMPTY)).toBe("?lang=en");
    expect(writeView("", EMPTY)).toBe("");
  });

  it("leaves out what is malformed and never picks half an hour of the week", () => {
    expect(readView("?from=yesterday&to=2030-13-45x&kind=road%20work,roadWorks&day=9&hour=8")).toEqual({ ...EMPTY, kinds: ["roadWorks"] });
    expect(readView("?kind=snow%2Fice,a%3Cb")).toEqual({ ...EMPTY, kinds: ["snow/ice"] });
    expect(readView("?day=2")).toEqual(EMPTY);
    expect(readView("?day=2&hour=24")).toEqual(EMPTY);
  });
});

describe("the filter the module applies", () => {
  it("starts a day at midnight in Helsinki, summer and winter", () => {
    expect(helsinkiMidnight("2030-07-01")).toBe(Date.parse("2030-06-30T21:00:00Z"));
    expect(helsinkiMidnight("2030-01-01")).toBe(Date.parse("2029-12-31T22:00:00Z"));
    expect(helsinkiMidnight("")).toBeUndefined();
  });

  it("keeps the last day whole and asks nothing that was not chosen", () => {
    expect(filterOf(EMPTY)).toEqual({});
    expect(filterOf({ ...EMPTY, to: "2030-10-07" })).toEqual({ to: Date.parse("2030-10-07T21:00:00Z") });
    expect(filterOf({ from: "2030-10-01", to: "", kinds: ["roadWorks"], weekday: 0, hour: 8 })).toEqual({
      from: Date.parse("2030-09-30T21:00:00Z"),
      subCategories: ["roadWorks"],
      weekday: 0,
      hour: 8,
    });
  });
});
