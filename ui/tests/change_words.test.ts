// covers (T-2137, the module gate in gate_modules.test.ts): src/routes/changeWords.ts.
/** T-3274: a change and each field it touched, in words. */
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { changeSentence, fieldSentence, shortValue } from "../src/routes/changeWords";

const t = (key: string, options?: Record<string, unknown>): string => String(i18n.t(key, options as never));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("a change in words (T-3274)", () => {
  it("says what was done when it merged and what was proposed when it did not", () => {
    const change = (phase: string, did: string) => ({
      summary: { key: `change.summary.${did}`, params: { kind: "Policy", name: "air-read" } },
      author: { name: "Jana" },
      status: { phase },
    });
    expect(changeSentence(change("Merged", "create"), t)).toBe("Jana created Policy air-read");
    expect(changeSentence(change("Applied", "delete"), t)).toBe("Jana removed Policy air-read");
    expect(changeSentence(change("Rejected", "update"), t)).toBe("Jana proposed to change Policy air-read");
  });

  it("says each field as set, removed or changed, with a long value cut short", () => {
    expect(fieldSentence({ path: "spec.validity.to", to: "2026-12-31" }, t)).toBe("set To to 2026-12-31");
    expect(fieldSentence({ path: "spec.q", from: "pm10>20" }, t)).toBe("removed Q (was pm10>20)");
    expect(fieldSentence({ path: "spec.operations[0]", from: ["a"], to: ["a", "b"] }, t)).toBe('changed Operations from ["a"] to ["a","b"]');
    expect(shortValue("x".repeat(100))).toHaveLength(60);
  });
});
