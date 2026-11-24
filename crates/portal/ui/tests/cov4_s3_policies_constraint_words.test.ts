// T-7706: constraintWords reads every shape of Policy filter, and keeps the text of a shape it cannot read.
import { beforeAll, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { constraintInWords, durationWords } from "../src/pages/policies/constraintWords";

const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);

describe("constraintInWords", () => {
  beforeAll(async () => {
    await i18n.changeLanguage("en");
  });

  it("reads a member without a name as nothing and keeps the text", () => {
    expect(constraintInWords("temporalQ", "=after;timerel", t, "en")).toBe(t("policies.sentence.temporal", { temporal: "=after;timerel" }));
  });

  it("keeps text of a duration that is empty or malformed", () => {
    expect(durationWords("P", "en")).toBeUndefined();
    expect(durationWords("-P", "en")).toBeUndefined();
    expect(durationWords("PT", "en")).toBeUndefined();
    expect(durationWords("P0D", "en")).toBeUndefined();
    expect(durationWords("nonsense", "en")).toBeUndefined();
    expect(durationWords("P-1D", "en")).toBe("1 day");
    expect(durationWords("PT-1H30M", "en")).toBe("1 hour and 30 minutes");
  });

  it("reads a window counted back, older, since, until and between", () => {
    expect(constraintInWords("temporalQ", "timerel=after;timeAt=P-1D", t, "en")).toContain("history up to 1 day back");
    expect(constraintInWords("temporalQ", "timerel=before;timeAt=P-7D", t, "en")).toContain("history older than 7 days");
    expect(constraintInWords("temporalQ", "timerel=after;timeAt=2026-01-02", t, "en")).toContain("history since");
    expect(constraintInWords("temporalQ", "timerel=before&timeAt=2026-01-02T10:00:00Z", t, "en")).toContain("history before");
    expect(constraintInWords("temporalQ", "timerel=between;timeAt=2026-01-02;endTimeAt=2026-02-03", t, "en")).toContain("history from");
  });

  it("keeps the text when a temporal filter has no reading", () => {
    const keep = (text: string) => expect(constraintInWords("temporalQ", text, t, "en")).toBe(t("policies.sentence.temporal", { temporal: text }));
    keep("timerel=between;timeAt=2026-01-02;endTimeAt=soon");
    keep("timerel=between;timeAt=nope;endTimeAt=2026-02-03");
    keep("timerel=after;timeAt=P1D");
    keep("timerel=after;timeAt=9999-99-99");
    keep("timerel=during;timeAt=2026-01-02");
    keep("timerel=after;timeAt=2026-01-02;timeproperty=modifiedAt");
    keep("timerel=after;timeAt=2026-01-02;lastN=3");
  });

  it("reads observedAt as the default time property", () => {
    expect(constraintInWords("temporalQ", "timerel=after;timeAt=P-2D;timeproperty=observedAt", t, "en")).toContain("2 days");
  });

  it("reads a scope below, a plain scope and keeps a scope it cannot read", () => {
    expect(constraintInWords("scopeQ", "/city/#", t, "en")).toBe(t("access.constraint.scopeBelow", { scope: "/city" }));
    expect(constraintInWords("scopeQ", "/#", t, "en")).toBe(t("policies.sentence.scope", { scope: "/#" }));
    expect(constraintInWords("scopeQ", "/city", t, "en")).toBe(t("policies.sentence.scope", { scope: "/city" }));
    expect(constraintInWords("scopeQ", "/a/+/b", t, "en")).toBe(t("policies.sentence.scope", { scope: "/a/+/b" }));
  });

  it("reads a geo filter as a distance from a point, inside or touching an area", () => {
    const point = "geometry=Point&coordinates=[17.1,48.1]";
    expect(constraintInWords("geoQ", `georel=near;maxDistance==500&${point}`, t, "en")).toContain("500 m");
    expect(constraintInWords("geoQ", `georel=near;maxDistance==2500&${point}`, t, "en")).toContain("2.5 km");
    expect(constraintInWords("geoQ", `georel=within&${point}`, t, "en")).toContain("inside the point 48.1, 17.1");
    expect(constraintInWords("geoQ", "georel=intersects&geometry=Polygon&coordinates=[[[0,0],[1,1],[0,1],[0,0]]]", t, "en")).toContain("touching a drawn area");
    expect(constraintInWords("geoQ", "georel=within&geometry=MultiPolygon&coordinates=[]", t, "en")).toContain("inside a drawn area");
  });

  it("keeps the text of a geo filter it cannot read", () => {
    const keep = (text: string) => expect(constraintInWords("geoQ", text, t, "en")).toBe(t("policies.sentence.geo", { geo: text }));
    keep("georel=near;maxDistance==500&geometry=Point&coordinates=[1,2");
    keep("georel=near;maxDistance==500&geometry=Line&coordinates=[1,2]");
    keep("georel=near;maxDistance==500&geometry=Point&coordinates=[1,\"x\"]");
    keep("georel=near;maxDistance==0&geometry=Point&coordinates=[1,2]");
    keep("georel=disjoint&geometry=Point&coordinates=[1,2]");
    keep("georel=near;maxDistance==500&geometry=Polygon&coordinates=[]");
  });

  it("reads a q filter with the model's labels", () => {
    expect(constraintInWords("q", "pm10>=40;today<x", t, "en", { pm10: "PM10" })).toContain("PM10 ≥ 40");
  });
});
