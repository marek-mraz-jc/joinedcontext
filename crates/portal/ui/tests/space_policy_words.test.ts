import { describe, expect, it } from "vitest";
import { assigneeWords, operationKind, operationKinds } from "../src/pages/spaces/policyWords";

const t = (key: string, options?: Record<string, unknown>) =>
  options ? `${key}(${Object.entries(options).map(([k, v]) => `${k}=${String(v)}`).join(",")})` : key;

describe("a space's policies in words", () => {
  it("folds CIM 009 operations into a few kinds, in a fixed order", () => {
    const kinds = operationKinds([
      "deleteBatch",
      "upsertBatch",
      "queryTemporal",
      "retrieveEntity",
      "createSubscription",
      "retrieveEntityTypes",
      "queryBatch",
    ]);
    expect(kinds.map((one) => one.kind)).toEqual(["read", "history", "write", "delete", "subscribe", "types"]);
    expect(kinds[0]?.operations).toEqual(["retrieveEntity", "queryBatch"]);
  });

  it("reads a group by what it stands for and an unknown name as other", () => {
    expect(operationKind("retrieveOps")).toEqual(["read"]);
    expect(operationKind("redirectionOps")).toEqual(["write", "delete"]);
    expect(operationKind("deleteSubscription")).toEqual(["subscribe"]);
    expect(operationKind("frobnicate")).toEqual(["other"]);
    expect(operationKinds([])).toEqual([]);
  });

  it("names the assignee in words, never as kind:id", () => {
    expect(assigneeWords({ kind: "role", id: "public" }, t)).toBe("spaces.inside.policyPublic");
    expect(assigneeWords({ kind: "role", id: "endpoint:helsinki/app-kpi" }, t)).toBe(
      "access.words.endpointRole(endpoint=app-kpi)",
    );
    expect(assigneeWords({ kind: "serviceAccount", id: "pipelines" }, t)).toContain("policies.sentence.who.serviceAccount");
    expect(assigneeWords(undefined, t)).toBe("—");
  });
});
