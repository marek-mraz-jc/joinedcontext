/**
 * T-7589: coverage for src/pages/mcp/aiConnectors.ts.
 * Exercises legacy connector conversion with kind-level change verbs,
 * form problem validations, ceiling checks, client refusal mapping,
 * and scope formatting.
 */
import { describe, expect, it } from "vitest";
import { ceilingAllows, clientRefusal, emptyForm, formProblems, kindRows, lastDay, pickKey, readsShared, refusalHolding, refusedSection, scopeLines, setup, toForm, toRequest, verbsOf, type AiConnector, type ConnectorOptions, type Pick } from "../src/pages/mcp/aiConnectors";

const NOW = new Date("2026-10-28T12:00:00Z");

const OPTIONS: ConnectorOptions = {
  segments: ["context", "pipeline"],
  segmentKinds: [
    { segment: "context", kinds: ["Endpoint", "ContextSpace"] },
    { segment: "pipeline", kinds: ["Pipeline"] },
  ],
  ceiling: {
    segments: ["context", "pipeline"],
    writes: true,
    maxDays: 30,
    kinds: [
      { kind: "Endpoint", verbs: ["create", "edit"] },
      { kind: "Pipeline", verbs: ["run"] },
    ],
  },
  projects: [{ project: "doprava", spaces: ["parking"], endpoints: ["bike-counters"], pipelines: ["import-pipe"], allowed: true }],
  clients: [
    { id: "mcp-local", isBoundary: false },
    { id: "https://chatgpt.com/connector/oauth/client.json", isBoundary: false },
  ],
};

const LEGACY_CONNECTOR: AiConnector = {
  id: "legacy-conn-1234567890abcdef",
  name: "legacy-conn",
  owner: "eva.steward",
  segments: ["context", "pipeline"],
  scope: [
    { project: "doprava", space: "parking", access: "write" },
    { project: "doprava", endpoint: "bike-counters", access: "read" },
  ],
  writes: true,
  autonomous: true,
  expiresAt: "2026-11-28T23:59:59Z",
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  limits: { parallelCalls: 4, resultBytes: 1048576 },
  state: "active",
  createdAt: "2026-10-28T12:00:00Z",
  lastUsedAt: null,
  url: "https://portal.dev.example/c/legacy-conn-1234567890abcdef",
};

describe("toForm legacy fallback", () => {

  it("handles legacy connector with undefined options gracefully", () => {
    const form = toForm(LEGACY_CONNECTOR, undefined);
    expect(form.verbs).toEqual({});
  });

  it("handles connector holding modern kinds list including propose expansion and run filtering", () => {
    const modern: AiConnector = {
      ...LEGACY_CONNECTOR,
      kinds: [
        { kind: "Endpoint", verbs: ["propose", "read"] },
        { kind: "Pipeline", verbs: ["run", "read"] },
        { kind: "ContextSpace", verbs: ["read"] },
      ],
    };
    const form = toForm(modern, OPTIONS);
    expect(form.verbs.Endpoint).toEqual(["create", "edit", "delete"]);
    expect(form.verbs.Pipeline).toEqual(["run"]);
    expect(form.verbs.ContextSpace).toBeUndefined();
  });
});

describe("verbsOf, kindRows, ceilingAllows, pickKey", () => {
  it("includes run verb only for Pipeline", () => {
    expect(verbsOf("Pipeline")).toEqual(["create", "edit", "delete", "run"]);
    expect(verbsOf("Endpoint")).toEqual(["create", "edit", "delete"]);
  });

  it("deduplicates kinds across segments and drops empty segments", () => {
    const opts: ConnectorOptions = {
      ...OPTIONS,
      segmentKinds: [
        { segment: "context", kinds: ["Endpoint", "DataModel"] },
        { segment: "pipeline", kinds: ["Endpoint", "Pipeline"] },
        { segment: "unused", kinds: ["App"] },
      ],
    };
    const rows = kindRows(opts, ["context", "pipeline"]);
    expect(rows).toEqual([
      { segment: "context", kinds: ["Endpoint", "DataModel"] },
      { segment: "pipeline", kinds: ["Pipeline"] },
    ]);
  });

  it("checks ceilingAllows correctly", () => {
    expect(ceilingAllows(OPTIONS, "Endpoint", "create")).toBe(true);
    expect(ceilingAllows(OPTIONS, "Endpoint", "delete")).toBe(false);
    expect(ceilingAllows(OPTIONS, "UnknownKind", "create")).toBe(false);
  });

  it("formats pick keys for projects and named items", () => {
    const projPick: Pick = { project: "helsinki", kind: "project", access: "read" };
    expect(pickKey(projPick)).toBe("helsinki");
    const spacePick: Pick = { project: "helsinki", kind: "space", name: "traffic", access: "write" };
    expect(pickKey(spacePick)).toBe("helsinki/space/traffic");
    const unnamedPick: Pick = { project: "helsinki", kind: "endpoint", access: "read" };
    expect(pickKey(unnamedPick)).toBe("helsinki/endpoint/");
  });
});

describe("emptyForm, lastDay, toRequest, formProblems", () => {
  it("populates emptyForm with default segment and maxDays expiry", () => {
    const form = emptyForm(OPTIONS, NOW);
    expect(form.name).toBe("");
    expect(form.segments).toEqual(["context"]);
    expect(form.expires).toBe(lastDay(30, NOW));
    expect(form.autonomous).toBe(false);

    const emptyNoOpts = emptyForm(undefined, NOW);
    expect(emptyNoOpts.segments).toEqual([]);
    expect(emptyNoOpts.expires).toBe(lastDay(30, NOW));
  });

  it("builds toRequest ensuring autonomous is only set when writes are true", () => {
    const form = emptyForm(OPTIONS, NOW);
    form.name = "my-connector";
    form.segments = ["context"];
    form.picks = [{ project: "doprava", kind: "endpoint", name: "bike-counters", access: "write" }];
    form.autonomous = true;
    form.verbs = { Endpoint: ["create"] };

    const req = toRequest(form, OPTIONS);
    expect(req.writes).toBe(true);
    expect(req.autonomous).toBe(true);
    expect(req.kinds).toEqual([
      { kind: "Endpoint", verbs: ["read", "create"] },
      { kind: "ContextSpace", verbs: ["read"] },
    ]);

    const reqWithoutOpts = toRequest(form, undefined);
    expect(reqWithoutOpts.kinds).toEqual([]);

    const reqReadonlyAutonomous = toRequest({ ...form, picks: [{ project: "doprava", kind: "endpoint", name: "bike-counters", access: "read" }] }, OPTIONS);
    expect(reqReadonlyAutonomous.writes).toBe(false);
    expect(reqReadonlyAutonomous.autonomous).toBe(false);
  });

  it("flags formProblems on bad name, empty segments, empty picks, or invalid dates", () => {
    const form = emptyForm(OPTIONS, NOW);
    form.name = "INVALID_NAME";
    form.segments = [];
    form.picks = [];
    form.expires = "2020-01-01";
    let problems = formProblems(form, OPTIONS, NOW);
    expect(problems.name).toBe("aiConnectors.problem.name");
    expect(problems.segments).toBe("aiConnectors.problem.segments");
    expect(problems.scope).toBe("aiConnectors.problem.scope");
    expect(problems.expiresAt).toBe("aiConnectors.problem.expires");

    form.expires = "2099-01-01";
    problems = formProblems(form, OPTIONS, NOW);
    expect(problems.expiresAt).toBe("aiConnectors.problem.expires");

    form.name = "valid-name";
    form.segments = ["context"];
    form.picks = [{ project: "doprava", kind: "project", access: "read" }];
    form.expires = lastDay(30, NOW);
    expect(formProblems(form, OPTIONS, NOW)).toEqual({});
  });
});

describe("refusedSection, scopeLines, readsShared", () => {
  it("extracts refusedSection from errors array or detail regex", () => {
    expect(refusedSection(["name"], "detail")).toBe("name");
    expect(refusedSection(["writes[0]"], "detail")).toBe("scope");
    expect(refusedSection(["kinds[0].verbs"], "detail")).toBe("kinds");
    expect(refusedSection(undefined, "segments: not allowed")).toBe("segments");
    expect(refusedSection(undefined, "allowedClients[0]: not allowed")).toBe("allowedClients");
    expect(refusedSection(undefined, "unknownField: bad")).toBeUndefined();
  });

  it("maps scope items to scope lines", () => {
    const lines = scopeLines([
      { project: "city", space: "parking", access: "write" },
      { project: "city", endpoint: "sensors", access: "read" },
      { project: "city", pipeline: "sync", access: "read" },
      { project: "city", access: "read" },
    ]);
    expect(lines).toEqual([
      { where: "city", kind: "space", name: "parking", access: "write" },
      { where: "city", kind: "endpoint", name: "sensors", access: "read" },
      { where: "city", kind: "pipeline", name: "sync", access: "read" },
      { where: "city", kind: "project", name: undefined, access: "read" },
    ]);
  });

  it("checks readsShared accurately", () => {
    const sharedOpts: ConnectorOptions = {
      ...OPTIONS,
      projects: [
        { project: "shared-city", spaces: [], endpoints: ["ep1"], pipelines: [], allowed: true, organizationData: true },
        { project: "private-city", spaces: [], endpoints: ["ep2"], pipelines: [], allowed: true, organizationData: false },
      ],
    };
    expect(readsShared({ ...LEGACY_CONNECTOR, scope: [{ project: "shared-city", endpoint: "ep1", access: "read" }] }, sharedOpts)).toBe(true);
    expect(readsShared({ ...LEGACY_CONNECTOR, scope: [{ project: "private-city", endpoint: "ep2", access: "read" }] }, sharedOpts)).toBe(false);
    expect(readsShared({ ...LEGACY_CONNECTOR, scope: [] }, sharedOpts)).toBe(false);
    expect(readsShared(LEGACY_CONNECTOR, undefined)).toBe(false);
  });
});

describe("clientLabel, namedClients, clientRefusal, refusalHolding, setup", () => {

  it("builds clientRefusal and checks refusalHolding", () => {
    const t = (k: string, v?: Record<string, string>) => (v ? `${k}:${JSON.stringify(v)}` : k);
    const clients = [
      { id: "client-a", refused: null },
      { id: "client-b", refused: "denied" },
    ];
    const denied = clientRefusal("does not allow the client client-a", clients, t);
    expect(denied.id).toBe("client-a");
    expect(denied.text).toContain("aiConnectors.noClientAllowed");

    const form = emptyForm(OPTIONS, NOW);
    form.clients = ["client-a"];
    expect(refusalHolding("does not allow the client client-a", form, clients, t)).toBeDefined();

    form.clients = ["client-other"];
    expect(refusalHolding("does not allow the client client-a", form, clients, t)).toBeUndefined();
    expect(refusalHolding(undefined, form, clients, t)).toBeUndefined();
  });

  it("formats setup commands for claude and vscode", () => {
    const s = setup("my-conn", "https://api.example.com/c/123");
    expect(s.claudeCode).toBe("claude mcp add --transport http my-conn https://api.example.com/c/123");
    const parsed = JSON.parse(s.vscode) as { servers: { [k: string]: { type: string; url: string } } };
    expect(parsed.servers["my-conn"]).toEqual({ type: "http", url: "https://api.example.com/c/123" });
  });
});
