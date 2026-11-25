// T-7706: j and k do nothing, and say so, on a page with no row link; markSeen keeps nothing when no entry exists.
import { afterEach, describe, expect, it } from "vitest";
import { act } from "../src/navigation/shortcuts";
import { markSeen, WHATS_NEW, lastSeen } from "../src/whatsNew";

afterEach(() => localStorage.clear());

describe("row shortcuts", () => {
  it("are not taken on a page with no row to move to", () => {
    const doc = document.implementation.createHTMLDocument("empty");
    expect(act(new KeyboardEvent("keydown", { key: "j" }), doc)).toBe(false);
    expect(act(new KeyboardEvent("keydown", { key: "k" }), doc)).toBe(false);
  });
});

describe("what's new, with nothing new", () => {
  it("marks nothing as seen when there is no entry", () => {
    const kept = WHATS_NEW.splice(0);
    try {
      markSeen();
      expect(lastSeen()).toBeUndefined();
    } finally {
      WHATS_NEW.push(...kept);
    }
  });
});
