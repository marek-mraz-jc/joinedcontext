import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { digest, movedPins, suggestVersion, usedBy, versionProblem, withVersion } from "../src/pages/scripts/scriptDraft";
import type { ScriptSpec } from "../src/pages/scripts/scriptDraft";

const spec: ScriptSpec = {
  language: "javascript",
  versions: [{ version: "1.0.0", state: "published", code: "x", sha256: "s" }],
};

const pipeline = (name: string, version: string): Manifest =>
  ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name, namespace: "helsinki" },
    spec: {
      steps: [{ kind: "bloblang", bloblang: "root = this" }, { kind: "script", scriptRef: { name: "flag", version } }],
      scripts: [{ scriptRef: { name: "other", version: "2.0.0" }, applyTo: "steps-start" }],
    },
  }) as unknown as Manifest;

describe("scriptDraft (T-7084)", () => {
  it("suggests a minor step when an output changed, a patch step otherwise, 1.0.0 first", () => {
    expect(suggestVersion(spec, true)).toBe("1.1.0");
    expect(suggestVersion(spec, false)).toBe("1.0.1");
    expect(suggestVersion(undefined, true)).toBe("1.0.0");
  });

  it("refuses a version that is not semver or not above the newest", () => {
    expect(versionProblem(spec, "1.1")).toBe("format");
    expect(versionProblem(spec, "01.1.0")).toBe("format");
    expect(versionProblem(spec, "1.0.0")).toBe("order");
    expect(versionProblem(spec, "1.1.0")).toBeUndefined();
  });

  it("takes the SHA-256 the kind checks", async () => {
    expect(await digest("abc")).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  });

  it("appends a version and keeps the published ones", () => {
    const stored = { metadata: { name: "flag", namespace: "helsinki" }, spec } as unknown as Manifest;
    const next = withVersion("helsinki", "flag", stored, { version: "1.1.0", state: "published", code: "y", sha256: "t" });
    expect((next.spec as unknown as ScriptSpec).versions.map((v) => v.version)).toEqual(["1.0.0", "1.1.0"]);
  });

  it("lists the pipelines pinning a script and moves only that script's pins", () => {
    const pipelines = [pipeline("a", "1.0.0"), pipeline("b", "1.1.0")];
    expect(usedBy(pipelines, "flag")).toEqual([
      { pipeline: "a", versions: ["1.0.0"] },
      { pipeline: "b", versions: ["1.1.0"] },
    ]);
    const moved = movedPins(pipelines[0], "flag", "1.1.0");
    expect(usedBy([moved], "flag")).toEqual([{ pipeline: "a", versions: ["1.1.0"] }]);
    expect(usedBy([moved], "other")).toEqual([{ pipeline: "a", versions: ["2.0.0"] }]);
  });
});
