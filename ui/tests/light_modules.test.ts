// covers: src/components/entities/reason.ts, src/schemas/dns1123.ts. T-5165: the public form and
// view pages take these two checks from modules of their own instead of the explorer's filters and
// every kind's schema; the old homes still answer with the same values.
import { describe, expect, it } from "vitest";
import { MAX_REASON, reasonProblem } from "../src/components/entities/reason";
import * as filters from "../src/components/entities/filters";
import { DNS1123 } from "../src/schemas/dns1123";
import * as kinds from "../src/schemas/kinds";

describe("the light checks of the public pages", () => {
  it("judges a reason as the gateway does: required, at most 500 characters once trimmed", () => {
    expect(reasonProblem("   ")).toBe("required");
    expect(reasonProblem(" fixed a typo ")).toBeNull();
    expect(reasonProblem("é".repeat(MAX_REASON))).toBeNull();
    expect(reasonProblem("x".repeat(MAX_REASON + 1))).toBe("tooLong");
    expect(filters.reasonProblem).toBe(reasonProblem);
    expect(filters.MAX_REASON).toBe(MAX_REASON);
  });

  it("takes an RFC 1123 label as a name and nothing else", () => {
    const name = new RegExp(DNS1123);
    expect(name.test("bb-pocasie")).toBe(true);
    expect(name.test("a")).toBe(true);
    for (const wrong of ["-a", "a-", "A", "a_b", ""]) expect(name.test(wrong), wrong).toBe(false);
    expect(kinds.DNS1123).toBe(DNS1123);
  });
});
