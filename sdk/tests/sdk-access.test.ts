import { describe, expect, it } from "vitest";
import { can, parseAccess } from "../src/sdk/access";
import type { AccessDocument } from "../src/sdk/access";

describe("sdk access", () => {
  it("answers checking reason when access is null", () => {
    const res = can(null, "queryEntity", "AirQualityObserved");
    expect(res).toEqual({ ok: false, reason: "Checking your permissions…" });
  });

  it("permits unconstrained access with wildcards", () => {
    const access: AccessDocument = {
      permissions: [{ resource: { type: "*" }, actions: ["*"], attributes: "*" }],
      prohibitions: [],
    };
    expect(can(access, "queryEntity", "Station")).toEqual({ ok: true });
    expect(can(access, "updateEntity", "Station", "bikes")).toEqual({ ok: true });
  });

  it("answers attribute-specific reason when operation is allowed but attribute is not", () => {
    const access: AccessDocument = {
      permissions: [
        {
          resource: { type: "Station" },
          actions: ["updateEntity"],
          attributes: ["status"],
        },
      ],
      prohibitions: [],
    };

    expect(can(access, "updateEntity", "Station", "status")).toEqual({ ok: true });
    expect(can(access, "updateEntity", "Station", "availableBikeNumber")).toEqual({
      ok: false,
      reason: "Your role may not change availableBikeNumber of Station.",
    });
    expect(can(access, "deleteEntity", "Station")).toEqual({
      ok: false,
      reason: "Your role may not deleteEntity Station.",
    });
  });

  it("prohibition overrides matching permission", () => {
    const access: AccessDocument = {
      permissions: [{ resource: { type: "Station" }, actions: ["*"], attributes: "*" }],
      prohibitions: [
        {
          resource: { type: "Station" },
          actions: ["deleteEntity"],
          attributes: "*",
        },
      ],
    };

    expect(can(access, "queryEntity", "Station")).toEqual({ ok: true });
    expect(can(access, "deleteEntity", "Station")).toEqual({
      ok: false,
      reason: "Your role may not deleteEntity Station.",
    });
  });

  it("parseAccess tolerates missing or malformed fields", () => {
    expect(parseAccess(null)).toEqual({ permissions: [], prohibitions: [] });
    expect(parseAccess({})).toEqual({ permissions: [], prohibitions: [] });
    expect(parseAccess({ permissions: "invalid" })).toEqual({ permissions: [], prohibitions: [] });
  });

  // SDK-36: a refusal to a person holding roles names them; an allowed call and the loading state
  // are unchanged.
  it("names the person's roles in a refusal", () => {
    const access: AccessDocument = {
      permissions: [{ resource: { type: "Alert" }, actions: ["updateAttrs"], attributes: ["status"] }],
      prohibitions: [],
    };
    expect(can(access, "updateAttrs", "Alert", "status", ["viewer"])).toEqual({ ok: true });
    expect(can(access, "deleteEntity", "Alert", undefined, ["viewer"])).toEqual({
      ok: false,
      reason: "Your role viewer does not permit deleteEntity on Alert.",
    });
    expect(can(access, "updateAttrs", "Alert", "stewardNote", ["viewer", "auditor"])).toEqual({
      ok: false,
      reason: "Your roles viewer, auditor do not permit updateAttrs on stewardNote of Alert.",
    });
    expect(can(access, "deleteEntity", "Alert", undefined, [])).toEqual({
      ok: false,
      reason: "Your role may not deleteEntity Alert.",
    });
    expect(can(null, "queryEntity", "Alert", undefined, ["viewer"]).reason).toBe("Checking your permissions…");
  });
});
