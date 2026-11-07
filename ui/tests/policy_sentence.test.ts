// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/policies/policySentence.ts,
// src/pages/policies/constraintWords.ts.
/** T-3276: a Policy reads as a sentence built from the same values as its YAML. */
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { constraintInWords } from "../src/pages/policies/constraintWords";
import { policySentence } from "../src/pages/policies/policySentence";

const t = (key: string, options?: Record<string, unknown>): string => String(i18n.t(key, options as never));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("a Policy in words (T-3276)", () => {
  it("says who may do what to which types and attributes, where, and for how long", () => {
    const sentence = policySentence(
      {
        contextSpaceRef: "ovzdusie",
        assignee: { kind: "group", id: "air-team" },
        operations: ["retrieveOps", "createEntity"],
        information: [{ entities: [{ type: "AirQualityObserved" }, { type: "Device" }], propertyNames: ["pm10"], relationshipNames: ["refDevice"] }],
        q: "pm10>20",
        validity: { from: "2026-01-01T00:00:00Z", to: "2026-12-31T00:00:00Z" },
      },
      t,
      "en",
    );
    expect(sentence).toBe(
      "Members of the group air-team may read and create entity on AirQualityObserved and Device in the space ovzdusie, only the attributes pm10 and refDevice, where pm10 > 20, from Jan 1, 2026 until Dec 31, 2026.",
    );
  });

  it("says may not for a prohibition, every type when none is named, and nobody before a grantee is picked", () => {
    expect(policySentence({ effect: "prohibition", contextSpaceRef: "s", assignee: { kind: "role", id: "viewer" }, operations: ["deleteEntity"] }, t, "en")).toBe(
      "Holders of the role viewer may not delete entity on every type in the space s.",
    );
  });

  // T-4215 (photo audit T-4163, rule 1): an empty form says what to pick, and a grant without a
  // space says so in words, never "may nothing yet … in the space —".
  it("asks for the choices on an empty form and names a space not chosen yet in words", () => {
    expect(policySentence({}, t, "en")).toBe("Pick who, what and which space, and the grant reads here as a sentence.");
    expect(policySentence({ assignee: { kind: "role", id: "viewer" }, operations: ["deleteEntity"] }, t, "en")).toBe(
      "Holders of the role viewer may delete entity on every type, in a space not chosen yet.",
    );
    expect(policySentence({ effect: "prohibition", operations: ["deleteEntity"] }, t, "en")).not.toContain("—");
  });

  it("speaks the person's language", async () => {
    await i18n.changeLanguage("sk");
    const sentence = policySentence(
      { contextSpaceRef: "ovzdusie", assignee: { kind: "role", id: "editor" }, operations: ["retrieveOps"], validity: { to: "2026-12-31T00:00:00Z" } },
      t,
      "sk",
    );
    expect(sentence).toMatch(/^Držitelia roly editor smú .* v priestore ovzdusie, do /);
  });

  // T-5593: a filter reads in words, the NGSI-LD text only where no reading fits.
  it("says a time window, a scope and a distance in words", () => {
    const words = (name: "q" | "scopeQ" | "geoQ" | "temporalQ", value: string) => constraintInWords(name, value, t, "en");
    expect(words("temporalQ", "timerel=after;timeAt=P-1D")).toBe("history up to 1 day back");
    expect(words("temporalQ", "timerel=after&timeAt=-P7D")).toBe("history up to 7 days back");
    expect(words("temporalQ", "timerel=before;timeAt=PT-1H30M")).toBe("history older than 1 hour and 30 minutes");
    expect(words("temporalQ", "timerel=after;timeAt=2026-01-01")).toBe("history since Jan 1, 2026");
    expect(words("temporalQ", "timerel=between;timeAt=2026-01-01;endTimeAt=2026-02-01")).toBe("history from Jan 1, 2026 to Feb 1, 2026");
    expect(words("temporalQ", "timerel=after;timeAt=P1D")).toBe("for the time timerel=after;timeAt=P1D");
    expect(words("temporalQ", "timerel=after;timeAt=P-1D;timeproperty=modifiedAt")).toBe(
      "for the time timerel=after;timeAt=P-1D;timeproperty=modifiedAt",
    );
    expect(words("scopeQ", "/city/#")).toBe("within the scope /city and everything under it");
    expect(words("scopeQ", "/geo/SK/BB")).toBe("within the scope /geo/SK/BB");
    expect(words("geoQ", "georel=near;maxDistance==500&geometry=Point&coordinates=[17.1,48.1]")).toBe(
      "within 500 m of the point 48.1, 17.1",
    );
    expect(words("geoQ", "georel=disjoint;geometry=Point;coordinates=[1,2]")).toBe(
      "in the area georel=disjoint;geometry=Point;coordinates=[1,2]",
    );
    expect(constraintInWords("q", "pm10>=40", t, "en", { pm10: "PM10" })).toBe("where PM10 ≥ 40");
    const sentence = policySentence({ contextSpaceRef: "s", assignee: { kind: "role", id: "viewer" }, operations: ["queryTemporal"], temporalQ: "timerel=after;timeAt=P-1D" }, t, "en");
    expect(sentence).toContain("history up to 1 day back");
  });
});
