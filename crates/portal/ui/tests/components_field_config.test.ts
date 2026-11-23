// The Grafana field settings the dashboard panels read: overrides of every property kind, the
// matchers, the date units relative to today, the value mappings and the custom and duration units.
import { describe, expect, it } from "vitest";
import { displayValue, formatDate, formatUnit, mapValue, settingsFor } from "../src/components/panels/fieldConfig";
import type { FrameField } from "../src/components/panels/frames";

const field: FrameField = { name: "Value", type: "number", values: [1, 2] };

describe("settingsFor overrides", () => {
  it("applies decimals, min, max, mappings, thresholds and color, and skips values of the wrong type", () => {
    const thresholds = { mode: "absolute", steps: [{ value: null, color: "green" }] };
    const settings = settingsFor(
      {
        defaults: {},
        overrides: [
          {
            matcher: { id: "byType", options: "number" },
            properties: [
              { id: "decimals", value: 2 },
              { id: "min", value: 1 },
              { id: "max", value: 9 },
              { id: "mappings", value: [{ type: "value", options: { "1": { text: "one" } } }] },
              { id: "thresholds", value: thresholds },
              { id: "color", value: { mode: "fixed", fixedColor: "red" } },
              { id: "somethingElse", value: 1 },
            ],
          },
          {
            matcher: { id: "byFrameRefID", options: "A" },
            properties: [
              { id: "max", value: "ten" },
              { id: "mappings", value: "none" },
              { id: "thresholds", value: { steps: "x" } },
              { id: "color", value: null },
            ],
          },
        ],
      },
      field,
      "A",
    );
    expect(settings).toMatchObject({ decimals: 2, min: 1, max: 9, thresholds, color: { mode: "fixed", fixedColor: "red" } });
    expect(settings.mappings).toHaveLength(1);
  });

  it("matches by type, by refId and by regexp, never by an unknown matcher or another refId", () => {
    const unit = (id: string, options: unknown, refId?: string) =>
      settingsFor({ defaults: {}, overrides: [{ matcher: { id, options }, properties: [{ id: "unit", value: "ms" }] }] }, field, refId).unit;
    expect(unit("byType", "number")).toBe("ms");
    expect(unit("byType", "string")).toBeUndefined();
    expect(unit("byFrameRefID", "B", "B")).toBe("ms");
    expect(unit("byFrameRefID", "B", "A")).toBeUndefined();
    expect(unit("byRegexp", "^Val")).toBe("ms");
    expect(unit("byRegexp", 3)).toBeUndefined();
    expect(unit("byValue", "Value")).toBeUndefined();
  });
});

describe("formatDate", () => {
  const at = new Date(2026, 10, 8, 15, 4, 5).getTime();
  const otherDay = new Date(2026, 10, 9, 9).getTime();
  it("writes US and local dates, the date left out today", () => {
    expect(formatDate(at, "dateTimeAsUSNoDateIfToday", false, "en", at)).toBe("3:04:05 pm");
    expect(formatDate(at, "dateTimeAsUSNoDateIfToday", false, "en", otherDay)).toBe("11/08/2026 3:04:05 pm");
    expect(formatDate(at, "dateTimeAsLocal", false, "en")).toBe(new Date(at).toLocaleString("en"));
    expect(formatDate(at, "dateTimeAsLocalNoDateIfToday", false, "en", at)).toBe(new Date(at).toLocaleTimeString("en"));
    expect(formatDate(at, "dateTimeAsLocalNoDateIfToday", false, "en", otherDay)).toBe(new Date(at).toLocaleString("en"));
  });
});

describe("mapValue", () => {
  it("maps a range, a regex with its groups and every special match", () => {
    const range = [{ type: "range", options: { from: 10, to: 20, result: { text: "mid" } } }];
    expect(mapValue(range, 15)).toEqual({ text: "mid" });
    expect(mapValue(range, "12")).toEqual({ text: "mid" });
    expect(mapValue(range, 25)).toBeUndefined();
    expect(mapValue([{ type: "range", options: { result: { text: "any" } } }], 1e9)).toEqual({ text: "any" });
    const regex = [{ type: "regex", options: { pattern: "^pod-(\\w+)$", result: { text: "pod $1$2", color: "blue" } } }];
    expect(mapValue(regex, "pod-broker")).toEqual({ text: "pod broker", color: "blue" });
    expect(mapValue(regex, null)).toBeUndefined();
    expect(mapValue([{ type: "regex", options: { pattern: "^x" } }], "xy")).toEqual({ text: undefined });
    const special = (match: string, value: unknown) => mapValue([{ type: "special", options: { match, result: { text: match } } }], value)?.text;
    expect(special("null", undefined)).toBe("null");
    expect(special("nan", Number.NaN)).toBe("nan");
    expect(special("null+nan", null)).toBe("null+nan");
    expect(special("null+nan", Number.NaN)).toBe("null+nan");
    expect(special("true", true)).toBe("true");
    expect(special("false", false)).toBe("false");
    expect(special("empty", "")).toBe("empty");
    expect(special("true", false)).toBeUndefined();
  });
});

describe("formatUnit custom and duration units", () => {
  it("writes suffix:, prefix: and an unknown unit after the number", () => {
    expect(formatUnit(3, "suffix: req", 0)).toBe("3 req");
    expect(formatUnit(3, "prefix:~", 0)).toBe("~3");
    expect(formatUnit(3, "parsecs", 0)).toBe("3 parsecs");
  });

  it("writes a duration in its largest whole unit, zero in seconds and below a millisecond in µs", () => {
    expect(formatUnit(0, "dtdurationms", 0)).toBe("0 s");
    expect(formatUnit(90_000, "dtdurationms", 1)).toBe("1.5 min");
    expect(formatUnit(2, "dtdurations", 0)).toBe("2 s");
    expect(formatUnit(3 * 86_400, "dtdurations", 0)).toBe("3 d");
    expect(formatUnit(0.5, "dtdurationms", 0)).toBe("500 µs");
  });
});

describe("displayValue", () => {
  it("keeps a mapping's colour on an empty value that the mapping gives no text", () => {
    const shown = displayValue({ mappings: [{ type: "special", options: { match: "null", result: { color: "red" } } }], custom: {} }, null);
    expect(shown.text).toBe("–");
    expect(shown.color).toBeTruthy();
  });
});
