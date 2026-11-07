// T-3584, SDK-41, AP-109: a function's role check reads the person the Portal verified for the
// call, so a page that lies about its roles changes nothing.
import { describe, expect, it } from "vitest";
import { requireRole } from "../src/sdk/server";

describe("requireRole", () => {
  it("lets the role's holder through and answers 403 or 401 to everyone else", () => {
    const steward = { user: { id: "u1", roles: ["steward"] } };
    expect(requireRole(steward, "steward")).toBeUndefined();
    expect(requireRole(steward, "admin")).toMatchObject({ status: 403, body: { role: "admin", title: "Role Required" } });
    expect(requireRole({ user: { id: "u2" } }, "steward")?.status).toBe(403);
    expect(requireRole({ user: null }, "steward")).toMatchObject({ status: 401, body: { title: "Sign In Required" } });
  });
});
