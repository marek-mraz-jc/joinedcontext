// T-7706: the small pure modules of slice 4 (assistant/state, processorForm, ckanAccess, presetFilters, entity filters, scriptDraft, pipelineDebug, dataoffer, device keys, the kinds' dashboard form).
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FilterColumn } from "@joinedcontext/sdk";
import type { Manifest } from "../src/api/manifest";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { onOpenRequest, parseRun, trail, requestOpen, runSnapshot, rememberRun, takeEditRequest, type OpenIntent } from "../src/assistant/state";
import { grantsInstance } from "../src/pages/ckan/ckanAccess";
import { qProblem } from "../src/components/entities/filters";
import { filtersFromQ } from "../src/components/entities/presetFilters";
import { fromFormData, withHelp, withVariables } from "../src/pages/pipelines/processorForm";
import { usedBy } from "../src/pages/scripts/scriptDraft";
import { diffOf } from "../src/pages/pipelines/pipelineDebug";
import { fromDataOfferManifest, toDataOfferManifest } from "../src/schemas/dataoffer";
import { generateDeviceKey } from "../src/pages/mqttbrokers/devices";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  window.sessionStorage.clear();
  window.history.pushState({}, "", "/");
});

describe("the assistant's stored run when the browser refuses its storage", () => {
  it("reads no run and an unparsable stored text as no run", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("storage is blocked");
    });
    expect(runSnapshot()).toBeNull();
    expect(parseRun("{not json")).toBeNull();
    expect(parseRun(JSON.stringify({ project: "helsinki" }))).toBeNull();
    expect(parseRun(JSON.stringify({ project: "helsinki", runId: "r1" }))).toEqual({ project: "helsinki", runId: "r1" });
    // Writing is refused too: the dock lives for this page only and nothing throws.
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("storage is blocked");
    });
    expect(() => rememberRun({ project: "helsinki", runId: "r2" })).not.toThrow();
  });

  it("reads an empty trail when the stored text is not JSON, and no edit request without a window", () => {
    window.sessionStorage.setItem("jc.assistant.trail", "{not json");
    expect(trail()).toEqual([]);
    vi.stubGlobal("window", undefined);
    expect(takeEditRequest()).toBeNull();
  });

  it("takes an edit request only for the resource that was asked for", () => {
    window.history.pushState({}, "", "/projects/helsinki/pipelines?edit=air-quality");
    expect(takeEditRequest("other")).toBeNull();
    expect(takeEditRequest("air-quality")).toEqual({ name: "air-quality", manifest: null });
    window.history.pushState({}, "", "/projects/helsinki/pipelines");
    expect(takeEditRequest()).toBeNull();
  });

  it("hands an open request made before anyone listens to the first listener only", () => {
    requestOpen("build");
    const first: OpenIntent[] = [];
    const second: OpenIntent[] = [];
    const stopFirst = onOpenRequest((intent) => first.push(intent));
    const stopSecond = onOpenRequest((intent) => second.push(intent));
    expect(first).toEqual(["build"]);
    expect(second).toEqual([]);
    requestOpen();
    expect(first).toEqual(["build", "chat"]);
    expect(second).toEqual(["chat"]);
    stopFirst();
    stopSecond();
  });
});

describe("the step form's data and schema", () => {
  it("passes a non-text value of a YAML box through and leaves non-record nodes alone", () => {
    const schema = { type: "object", properties: { body: { type: "string" } } } as JsonSchema;
    const yamlUi = { "ui:options": { yaml: true } } as UiSchema;
    expect(fromFormData(42, schema, yamlUi)).toBe(42);
    expect(fromFormData(undefined, schema, yamlUi)).toBeUndefined();
    expect(fromFormData("   ", schema, yamlUi)).toBeUndefined();

    const ui = { note: "plain text", "ui:order": ["note"], pw: { "ui:options": { secret: true } } } as unknown as UiSchema;
    const helped = withHelp(ui, "name a variable") as Record<string, unknown>;
    expect(helped.note).toBe("plain text");
    expect((helped.pw as Record<string, unknown>)["ui:help"]).toBe("name a variable");

    const odd = {
      type: "object",
      properties: { flag: true, pw: { type: "string" } },
    } as unknown as JsonSchema;
    const offered = withVariables(odd, { pw: { "ui:options": { secret: true } } } as unknown as UiSchema, ["PW"]) as {
      properties: { flag: unknown; pw: { examples: string[] } };
    };
    expect(offered.properties.flag).toBe(true);
    expect(offered.properties.pw.examples).toEqual(["${PW}"]);
  });
});

describe("who may change a CKAN instance, by the role's constraints", () => {
  const role = (constraints: object[]): Manifest =>
    ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Role",
      metadata: { name: "r", namespace: "org" },
      spec: { rules: [{ kinds: ["CkanInstance"], verbs: ["approve"], constraints }] },
    }) as Manifest;

  it("reads a spec constraint as no narrowing of the name and an equals as exact", () => {
    expect(grantsInstance(role([{ field: "spec.title", equals: "x" }]), "any-instance")).toBe(true);
    expect(grantsInstance(role([{ field: "metadata.name", equals: "open-data" }]), "open-data")).toBe(true);
    expect(grantsInstance(role([{ field: "metadata.name", equals: "open-data" }]), "other")).toBe(false);
    expect(grantsInstance(role([{ field: "metadata.name", pattern: "open-.*" }]), "open-data")).toBe(true);
    expect(grantsInstance(role([{ field: "metadata.name", pattern: "(" }]), "open-data")).toBe(false);
    expect(grantsInstance(role([{ field: "metadata.name", notIn: ["open-data"] }]), "open-data")).toBe(false);
  });
});

describe("a default filter as the data view's rows", () => {
  const columns: FilterColumn[] = [
    { key: "status", attr: "status", kind: "enum", options: [{ value: "open", title: "open" }] },
    { key: "level", attr: "level", kind: "number" },
  ];

  it("shows a range written the other way round as one between row", () => {
    expect(filtersFromQ("level<=9;level>=2", columns)).toEqual({ level: { op: "between", value: "2", value2: "9" } });
    expect(filtersFromQ("level>=2;level<=9", columns)).toEqual({ level: { op: "between", value: "2", value2: "9" } });
  });

  it("gives up on two terms of one enum column, and on two terms that are no range", () => {
    expect(filtersFromQ('status=="open";status=="closed"', columns)).toBeNull();
    expect(filtersFromQ("level>1;level>2", columns)).toBeNull();
  });
});

describe("the check of a typed q", () => {
  it("calls a value with a padded space a shape problem, not a missing quote", () => {
    expect(qProblem("name== open")).toEqual({ kind: "shape" });
    expect(qProblem("name==open")).toEqual({ kind: "unquoted", fix: 'name=="open"' });
  });
});

describe("the pipelines that pin a script", () => {
  it("finds pins in steps, routes and the applied scripts list", () => {
    const pipeline = (name: string, spec: object): Manifest =>
      ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Pipeline", metadata: { name, namespace: "helsinki" }, spec }) as Manifest;
    const found = usedBy(
      [
        pipeline("a", { scripts: [{ scriptRef: { name: "clean", version: "1.2.0" } }, { note: "no pin" }] }),
        pipeline("b", { routes: [{ steps: [{ scriptRef: { name: "clean", version: "1.0.0" } }] }] }),
        pipeline("c", {}),
      ],
      "clean",
    );
    expect(found).toEqual([
      { pipeline: "a", versions: ["1.2.0"] },
      { pipeline: "b", versions: ["1.0.0"] },
    ]);
  });
});

describe("what a node did to a message", () => {
  it("says a whole message was removed or added", () => {
    expect(diffOf("text", undefined)).toEqual([{ path: "(message)", kind: "removed", before: "text" }]);
    expect(diffOf(undefined, "text")).toEqual([{ path: "(message)", kind: "added", after: "text" }]);
    expect(diffOf("a", "b")).toEqual([{ path: "(message)", kind: "changed", before: "a", after: "b" }]);
  });
});

describe("the data offer form", () => {
  it("stores an offer without a policy text as no policy, and an unparsable one as written", () => {
    const stored = toDataOfferManifest("helsinki", { name: "offer", contextSpaceRef: "bikes", endpointRefs: ["e1"] }) as {
      spec: { policy: unknown };
    };
    expect(stored.spec.policy).toBeUndefined();
    const broken = toDataOfferManifest("helsinki", { name: "offer", policy: "{not json" }) as { spec: { policy: unknown } };
    expect(broken.spec.policy).toBe("{not json");
    expect(fromDataOfferManifest(undefined).endpointRefs).toEqual([]);
  });
});

describe("a fresh device key", () => {
  it("refuses a key pair the browser cannot read back as a public key", async () => {
    vi.spyOn(crypto.subtle, "exportKey").mockResolvedValueOnce({ kty: "oct" } as JsonWebKey);
    await expect(generateDeviceKey()).rejects.toThrow("the browser made a key it cannot read back");
  });
});
