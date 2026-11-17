/**
 * F-UI1-021: an empty permissions answer is asked again while a fresh pod syncs (T-0630), and
 * the asking stops: a person who genuinely holds no grant is not polled for as long as the page
 * stays open.
 */
import { describe, expect, it } from "vitest";
import { ApiError } from "../src/api/client";
import { EMPTY_GRANTS_ASKS, deniedBy, emptyGrantsRetry } from "../src/api/permissions";

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

describe("deniedBy", () => {
  it("reads the Organization's rules with no grant as denied, as its old 404 was (T-7176)", () => {
    const none = { project: "org", bootstrap: false, grants: [] };
    expect(deniedBy("org", null, none)).toBe(true);
    expect(deniedBy("org", null, { ...none, bootstrap: true })).toBe(false);
    expect(deniedBy("org", null, undefined)).toBe(false);
    // A project's empty document is a fresh pod's answer, asked again, not a refusal.
    expect(deniedBy("air", null, { ...none, project: "air" })).toBe(false);
    expect(deniedBy("air", new ApiError(404, "project 'air' not found"), undefined)).toBe(true);
  });
});
