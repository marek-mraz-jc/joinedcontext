import { describe, expect, it } from "vitest";
import { historyKind, segmentsOf, valueText } from "../src/grid/historyKinds";

const at = (minute: number) => new Date(Date.UTC(2026, 10, 1, 12, minute)).toISOString();
const of = (...values: unknown[]) => values.map((value, i) => ({ at: at(i), value }));

describe("historyKind", () => {
  it("names each NGSI-LD value shape", () => {
    expect(historyKind(of(1, 2.5, "3"))).toBe("number");
    expect(historyKind(of(true, false))).toBe("boolean");
    expect(historyKind(of("working", "closed", "working"))).toBe("state");
    expect(historyKind(of("urn:ngsi-ld:A:1", "urn:ngsi-ld:A:2"))).toBe("relationship");
    expect(historyKind(of({ type: "Point", coordinates: [24.9, 60.1] }))).toBe("geo");
    expect(historyKind(of({ fi: "Kumpula", en: "Kumpula" }))).toBe("language");
    expect(historyKind(of({ a: 1 }, "x"))).toBe("structured");
    expect(historyKind(of(...Array.from({ length: 13 }, (_, i) => `t${i}`)))).toBe("text");
    expect(historyKind([])).toBeNull();
  });

  it("joins equal neighbours into one segment and runs the last to the end", () => {
    const end = Date.parse(at(10));
    expect(segmentsOf(of("a", "a", "b"), end)).toEqual([
      { value: "a", from: Date.parse(at(0)), to: Date.parse(at(2)) },
      { value: "b", from: Date.parse(at(2)), to: end },
    ]);
  });

  it("writes any value as one line", () => {
    const n = (x: number) => String(x);
    expect(valueText(false, n)).toBe("no");
    expect(valueText({ type: "Point", coordinates: [24.9, 60.1] }, n)).toBe("Point 24.9, 60.1");
    expect(valueText({ a: 1 }, n)).toBe('{"a":1}');
  });
});
