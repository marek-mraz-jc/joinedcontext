/** T-0221: what an edit costs, before it is saved (DM-23, DM-24, DM-25). */
import { describe, expect, it } from "vitest";
import { parseModel } from "../src/pages/models/linkml";
import {
  bumpVersion,
  classifyChanges,
  laneOf,
  refusalToSave,
  severityOf,
} from "../src/pages/models/breaking_detector";

const PUBLISHED = `
id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots: [pm10, station]
slots:
  pm10:
    range: float
    slot_uri: bb:pm10
    minimum_value: 0
  station:
    range: string
    slot_uri: bb:station
enums:
  QualityBand:
    permissible_values:
      good: {}
      poor: {}
`;

function edited(mutate: (source: string) => string) {
  return parseModel(mutate(PUBLISHED));
}

const published = parseModel(PUBLISHED);

describe("breaking change detector", () => {
  it("counts a new optional slot as additive and bumps the minor", () => {
    const next = edited((source) =>
      source.replace(
        "  station:\n    range: string\n    slot_uri: bb:station",
        "  station:\n    range: string\n    slot_uri: bb:station\n  pm25:\n    range: float\n    slot_uri: bb:pm25",
      ),
    );
    const changes = classifyChanges(published, next);
    expect(severityOf(changes)).toBe("additive");
    expect(bumpVersion("2.1.0", changes)).toBe("2.2.0");
    expect(changes.map((change) => change.subject)).toContain("pm25");
  });

  it("counts a new enum value as additive", () => {
    const next = edited((source) => source.replace("      poor: {}", "      poor: {}\n      moderate: {}"));
    expect(severityOf(classifyChanges(published, next))).toBe("additive");
  });

  it("counts a removed slot as breaking and bumps the major", () => {
    const next = edited((source) =>
      source.replace("    slots: [pm10, station]", "    slots: [pm10]"),
    );
    const changes = classifyChanges(published, next);
    expect(severityOf(changes)).toBe("breaking");
    expect(bumpVersion("2.1.0", changes)).toBe("3.0.0");
  });

  it("counts a changed IRI as breaking, because consumers resolve it", () => {
    const next = edited((source) => source.replace("slot_uri: bb:pm10", "slot_uri: bb:pm_10"));
    const changes = classifyChanges(published, next);
    expect(severityOf(changes)).toBe("breaking");
    expect(changes.some((change) => change.reason.includes("IRI"))).toBe(true);
  });

  it("counts a narrowed range and a raised minimum as breaking, a widened range as additive", () => {
    const narrowed = edited((source) =>
      source.replace("  station:\n    range: string", "  station:\n    range: integer"),
    );
    expect(severityOf(classifyChanges(published, narrowed))).toBe("breaking");

    const raised = edited((source) => source.replace("minimum_value: 0", "minimum_value: 5"));
    expect(severityOf(classifyChanges(published, raised))).toBe("breaking");

    const widened = edited((source) =>
      source.replace("  pm10:\n    range: float", "  pm10:\n    range: string"),
    );
    expect(severityOf(classifyChanges(published, widened))).toBe("additive");
  });

  it("counts a slot that becomes required as breaking, because every writer is now wrong", () => {
    const next = edited((source) =>
      source.replace("    slot_uri: bb:station", "    slot_uri: bb:station\n    required: true"),
    );
    expect(severityOf(classifyChanges(published, next))).toBe("breaking");
  });

  it("counts a removed enum value as breaking, because stored entities carry it", () => {
    const next = edited((source) => source.replace("      poor: {}\n", ""));
    expect(severityOf(classifyChanges(published, next))).toBe("breaking");
  });

  it("refuses to save a breaking change under the same major, and names the version to use", () => {
    const next = edited((source) => source.replace("slot_uri: bb:pm10", "slot_uri: bb:pm_10"));
    const changes = classifyChanges(published, next);
    expect(refusalToSave(changes, "2.1.0", "2.2.0")).toContain("3.0.0");
    expect(refusalToSave(changes, "2.1.0", "3.0.0")).toBeUndefined();
  });

  it("lets an additive change save under its own version", () => {
    const next = edited((source) => source.replace("      poor: {}", "      poor: {}\n      moderate: {}"));
    expect(refusalToSave(classifyChanges(published, next), "2.1.0", "2.2.0")).toBeUndefined();
  });

  it("puts a draft in green, an additive change to a published model in yellow, a break in red", () => {
    const additive = classifyChanges(
      published,
      edited((source) => source.replace("      poor: {}", "      poor: {}\n      moderate: {}")),
    );
    const breaking = classifyChanges(
      published,
      edited((source) => source.replace("    slots: [pm10, station]", "    slots: [pm10]")),
    );
    expect(laneOf(additive, "draft")).toBe("green");
    expect(laneOf(additive, "published")).toBe("yellow");
    expect(laneOf(breaking, "draft")).toBe("red");
    // A model a public endpoint serves is red however small the edit (DM-24, CC-63).
    expect(laneOf(additive, "published", true)).toBe("red");
  });

  it("names every other slot, class and enum change with its severity", () => {
    const lines = (mutate: (source: string) => string) =>
      classifyChanges(published, edited(mutate)).map((change) => `${change.severity} ${change.subject}: ${change.reason}`);
    const PM10 = "  pm10:\n    range: float\n    slot_uri: bb:pm10\n    minimum_value: 0";

    // A slot that gains a profile is only wider; one that loses it breaks the projections.
    expect(lines((source) => source.replace(PM10, `${PM10}\n    subsets: [core]`))).toEqual([
      "additive pm10: the profile core now takes this slot",
    ]);
    const profiled = parseModel(PUBLISHED.replace(PM10, `${PM10}\n    subsets: [core]`));
    expect(classifyChanges(profiled, published).map((change) => `${change.severity} ${change.reason}`)).toEqual([
      "breaking the profile core no longer takes this slot",
    ]);

    // A range that appears where there was none narrows the slot, one that goes widens it.
    expect(lines((source) => source.replace("  station:\n    range: string\n", "  station:\n"))).toEqual([
      "additive station: the range widened from string to none",
    ]);
    const none = parseModel(PUBLISHED.replace("  station:\n    range: string\n", "  station:\n"));
    expect(classifyChanges(none, published).map((change) => change.reason)).toEqual([
      "the range narrowed from none to string",
    ]);

    // An integer becomes a float: wider, never narrower.
    const integer = parseModel(PUBLISHED.replace("range: float", "range: integer"));
    expect(classifyChanges(integer, published).map((change) => `${change.severity} ${change.reason}`)).toEqual([
      "additive the range widened from integer to float",
    ]);

    expect(lines((source) => source.replace(PM10, `${PM10}\n    deprecated: true\n    pattern: "^[0-9]+$"\n    maximum_value: 9`))).toEqual([
      "additive pm10: the slot is marked deprecated, which keeps it valid and warns consumers",
      "breaking pm10: a pattern was added or tightened, so values that validated before may not",
      "breaking pm10: the maximum value was lowered",
    ]);
    expect(lines((source) => source.replace(PM10, `${PM10}\n    annotations:\n      ngsi_ld_kind: GeoProperty`))).toEqual([
      "breaking pm10: the NGSI-LD kind changed from Property to GeoProperty",
    ]);

    const required = parseModel(PUBLISHED.replace(PM10, `${PM10}\n    required: true`));
    expect(classifyChanges(required, published).map((change) => `${change.severity} ${change.reason}`)).toEqual([
      "additive the slot is no longer required",
    ]);

    expect(lines((source) => source.replace("    slots: [pm10, station]", "    slots: [pm10, station]\n  Station:\n    class_uri: bb:Station\n    slots: []"))).toEqual([
      "additive Station: a new class was added",
    ]);
    expect(lines((source) => source.replace("class_uri: bb:AirQualityObserved", "class_uri: bb:Air"))).toEqual([
      "breaking AirQualityObserved: the class IRI changed from bb:AirQualityObserved to bb:Air",
    ]);
    const bigger = parseModel(PUBLISHED.replace("    slots: [pm10, station]", "    slots: [pm10, station]\n  Station:\n    class_uri: bb:Station\n    slots: []"));
    expect(classifyChanges(bigger, published).map((change) => `${change.severity} ${change.subject}`)).toEqual([
      "breaking Station",
    ]);
    expect(lines((source) => source.replace(/enums:[\s\S]*$/, "enums:\n  Other:\n    permissible_values:\n      a: {}\n"))).toEqual([
      "breaking QualityBand: the enum was removed",
      "additive Other: a new enum was added",
    ]);
  });

  it("sees no change where nothing changed", () => {
    const changes = classifyChanges(published, parseModel(PUBLISHED));
    expect(changes).toEqual([]);
    expect(bumpVersion("2.1.0", changes)).toBe("2.1.0");
  });
});

/**
 * T-1094, DM-13: the classifier reads the hierarchy too. A class inherits its parent's and its
 * mixins' slots and a projection takes the slots of the profiles it names, so losing one takes
 * away what it brought — a reader of the old model holds attributes the new one no longer
 * promises. Gaining one only adds.
 */
describe("a change to the hierarchy the model declares", () => {
  const withHierarchy = `${PUBLISHED}
imports:
  - linkml:types
`;

  it("calls losing a parent, a mixin, a profile or an import breaking", () => {
    const before = parseModel(
      withHierarchy
        .replace("    class_uri: bb:AirQualityObserved", "    class_uri: bb:AirQualityObserved\n    is_a: Station\n    mixins: [Timed]")
        .replace("    minimum_value: 0", "    minimum_value: 0\n    subsets: [public]"),
    );
    const after = parseModel(PUBLISHED);
    const changes = classifyChanges(before, after);
    const reasons = changes.map((change) => change.reason).join(" | ");

    expect(severityOf(changes)).toBe("breaking");
    expect(reasons).toContain("specialises");
    expect(reasons).toContain("Timed");
    expect(reasons).toContain("public");
    expect(reasons).toContain("linkml:types");
  });

  it("calls gaining one additive", () => {
    const after = parseModel(
      withHierarchy.replace(
        "    class_uri: bb:AirQualityObserved",
        "    class_uri: bb:AirQualityObserved\n    mixins: [Timed]",
      ),
    );
    const changes = classifyChanges(parseModel(PUBLISHED), after);
    expect(severityOf(changes)).toBe("additive");
    expect(changes.map((change) => change.reason).join(" | ")).toContain("Timed");
  });

  it("says nothing when the hierarchy did not move", () => {
    expect(classifyChanges(parseModel(withHierarchy), parseModel(withHierarchy))).toEqual([]);
  });
});
