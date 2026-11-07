/**
 * F-UI1-021: an empty permissions answer is asked again while a fresh pod syncs (T-0630), and
 * the asking stops: a person who genuinely holds no grant is not polled for as long as the page
 * stays open.
 */
import { describe, expect, it } from "vitest";
import { EMPTY_GRANTS_ASKS, emptyGrantsRetry } from "../src/api/permissions";

describe("emptyGrantsRetry", () => {
  it("asks an empty answer again, then stops", () => {
    const empty = { grants: [] };
    expect(emptyGrantsRetry(empty, 1)).toBe(5000);
    expect(emptyGrantsRetry(empty, EMPTY_GRANTS_ASKS - 1)).toBe(5000);
    expect(emptyGrantsRetry(empty, EMPTY_GRANTS_ASKS)).toBe(false);
    expect(emptyGrantsRetry(empty, 500)).toBe(false);
  });

  it("never asks again once a grant or the bootstrap answer is in", () => {
    expect(emptyGrantsRetry({ grants: [{ kinds: ["*"] }] }, 1)).toBe(false);
    expect(emptyGrantsRetry({ grants: [], bootstrap: true }, 1)).toBe(false);
    expect(emptyGrantsRetry(undefined, 0)).toBe(false);
  });
});
