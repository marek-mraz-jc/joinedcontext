/** The one owner check of a workspace the bar, the list, Bring back and Try it share (T-6917). */
import { describe, expect, it } from "vitest";
import { isWorkspaceOwner } from "../src/pages/workspaces/owner";

describe("isWorkspaceOwner", () => {
  it("is the owner by email or by username", () => {
    expect(isWorkspaceOwner({ email: "jana@hel.fi", username: "jana" }, "jana@hel.fi")).toBe(true);
    expect(isWorkspaceOwner({ email: null, username: "jana" }, "jana")).toBe(true);
  });

  it("takes nobody for the owner of an empty or unknown owner, an email-less person included", () => {
    expect(isWorkspaceOwner({ username: "bot" }, undefined)).toBe(false);
    expect(isWorkspaceOwner({ email: null, username: "bot" }, "")).toBe(false);
    expect(isWorkspaceOwner(null, "jana")).toBe(false);
    expect(isWorkspaceOwner({ email: "eva@hel.fi", username: "eva" }, "jana@hel.fi")).toBe(false);
  });
});
