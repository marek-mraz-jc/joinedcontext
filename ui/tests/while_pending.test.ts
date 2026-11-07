import { describe, expect, it } from "vitest";
import { whilePending } from "../src/api/client";

const list = (...phases: unknown[]) => ({
  state: { data: { items: phases.map((phase) => (phase === null ? null : { status: { phase } })) } },
});

describe("whilePending", () => {
  it("polls every 10 s while an item is Pending or Deploying", () => {
    expect(whilePending(list("Live", "Pending"))).toBe(10_000);
    expect(whilePending(list("Deploying"))).toBe(10_000);
  });

  it("stops once every item has settled, and on an empty, missing or odd list", () => {
    expect(whilePending(list("Live", "Error", "Drifted", "Draft"))).toBe(false);
    expect(whilePending(list())).toBe(false);
    expect(whilePending({ state: {} })).toBe(false);
    expect(whilePending(list(null, undefined, 7))).toBe(false);
    expect(whilePending({ state: { data: { items: [{}] } } })).toBe(false);
  });
});
