// The typed OData client (T-6816): query building, paging on nextLink, If-Match on bound actions,
// 412/428 and the OData error object read as an API/09 problem, and the generated types matching
// the $metadata snapshot (the CI no-diff check).
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { generate, SNAPSHOT, TARGET } from "../scripts/odata-gen.mjs";
import { keyPredicate, literal, ODataError, odata } from "../src/sdk/odata/client";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";

/** A transport answering `answers` in turn and keeping each request. */
function scripted(...answers: JcResponse[]): { transport: Transport; sent: JcRequest[] } {
  const sent: JcRequest[] = [];
  const transport: Transport = async (request) => {
    sent.push(request);
    return answers.shift() ?? { status: 500, body: null };
  };
  return { transport, sent };
}

describe("the OData client", () => {
  it("builds a set's query with every option, encoded", async () => {
    const { transport, sent } = scripted({ status: 200, body: { value: [{ name: "pm10" }], "@odata.count": 7 } });
    const page = await odata("/bbsk/api/v1", { transport })
      .from("endpoints")
      .filter("project eq 'air' and phase eq 'Ready'")
      .select("name", "title")
      .orderBy("name")
      .orderBy("title", "desc")
      .top(50)
      .skip(100)
      .count()
      .page();
    expect(sent[0].path).toBe(
      "/bbsk/api/v1/endpoints?$filter=project%20eq%20'air'%20and%20phase%20eq%20'Ready'" +
        "&$select=name%2Ctitle&$orderby=name%2Ctitle%20desc&$top=50&$skip=100&$count=true",
    );
    expect(sent[0].headers).toEqual({ "OData-Version": "4.01" });
    expect(page).toEqual({ value: [{ name: "pm10" }], count: 7, nextLink: undefined });
  });

  it("writes keys and literals as OData does", () => {
    expect(literal("it's")).toBe("'it''s'");
    expect(literal(3)).toBe("3");
    expect(literal({ a: 1 })).toBe('{"a":1}');
    expect(keyPredicate("air")).toBe("('air')");
    expect(keyPredicate({ project: "air", name: "pm10" })).toBe("(project='air',name='pm10')");
  });

  it("pages on nextLink of its own service and refuses one elsewhere", async () => {
    const root = `${location.origin}/api/v1/`;
    const { transport, sent } = scripted(
      { status: 200, body: { value: [{ name: "a" }], "@odata.nextLink": `${root}projects?$skiptoken=x1` } },
      { status: 200, body: { value: [{ name: "b" }] } },
    );
    const names: string[] = [];
    for await (const row of odata("/api/v1/", { transport }).from("projects").all()) names.push(row.name);
    expect(names).toEqual(["a", "b"]);
    expect(sent.map((r) => r.path)).toEqual(["/api/v1/projects", "/api/v1/projects?$skiptoken=x1"]);

    const away = scripted({
      status: 200,
      body: { value: [], "@odata.nextLink": "https://evil.example/api/v1/projects?$skiptoken=x" },
    });
    const rows = odata("/api/v1/", { transport: away.transport }).from("projects").all();
    await expect(async () => {
      for await (const _ of rows) void _;
    }).rejects.toThrow(/outside this service/);
    expect(away.sent).toHaveLength(1);
  });

  it("calls a function with its parameters as aliases", async () => {
    const { transport, sent } = scripted({ status: 200, body: { value: { kind: "Endpoint" } } });
    const value = await odata("/api/v1/", { transport }).call("jc_schema_get", "air", {
      kind: "Endpoint",
      path: "/spec/a b",
    });
    expect(value).toEqual({ kind: "Endpoint" });
    expect(sent[0].method).toBe("GET");
    expect(sent[0].path).toBe(
      "/api/v1/projects('air')/JoinedContext.Portal.jc_schema_get(kind=@kind,path=@path)" +
        "?@kind='Endpoint'&@path='%2Fspec%2Fa%20b'",
    );
  });

  it("sends If-Match on a bound action and reads 428 and 412", async () => {
    const { transport, sent } = scripted(
      { status: 202, body: { name: "c-1" } },
      {
        status: 428,
        body: { error: { code: "precondition-required", message: "send If-Match", status: 428 } },
      },
      {
        status: 412,
        body: { error: { code: "precondition-failed", message: "changed", title: "Precondition Failed" } },
        headers: { etag: '"new"' },
      },
    );
    const client = odata("/api/v1/", { transport });
    const bound = "spaces(project='air',name='pm10')";
    await expect(client.act("propose_delete", bound, { dryRun: true }, '"old"')).resolves.toEqual({ name: "c-1" });
    expect(sent[0]).toMatchObject({
      method: "POST",
      path: "/api/v1/spaces(project='air',name='pm10')/JoinedContext.Portal.propose_delete",
      body: { dryRun: true },
      headers: { "OData-Version": "4.01", "If-Match": '"old"' },
    });
    const required = await client.act("propose_delete", bound, {}).catch((e: unknown) => e);
    expect(required).toBeInstanceOf(ODataError);
    expect((required as ODataError).preconditionRequired).toBe(true);
    const stale = (await client.act("propose_delete", bound, {}, '"old"').catch((e: unknown) => e)) as ODataError;
    expect([stale.stale, stale.etag, stale.code]).toEqual([true, '"new"', "precondition-failed"]);
  });

  it("maps the OData error object and a problem+json to one API/09 problem", async () => {
    const { transport } = scripted(
      {
        status: 404,
        body: {
          error: {
            code: "resource-not-found",
            message: "no project air you may read",
            type: "https://joinedcontext.com/errors/resource-not-found",
            title: "Not Found",
            status: 404,
          },
        },
      },
      {
        status: 403,
        body: { type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", detail: "not yours" },
      },
    );
    const client = odata("/api/v1/", { transport });
    const missing = (await client.get("projects", "air").catch((e: unknown) => e)) as ODataError;
    expect([missing.status, missing.type, missing.code, missing.message]).toEqual([
      404,
      "https://joinedcontext.com/errors/resource-not-found",
      "resource-not-found",
      "no project air you may read",
    ]);
    const refused = (await client.singleton("me").catch((e: unknown) => e)) as ODataError;
    expect([refused.status, refused.code, refused.message]).toEqual([403, "forbidden", "not yours"]);
  });

  it("refuses a compound key that misses a member before sending", async () => {
    const { transport, sent } = scripted();
    const client = odata("/api/v1/", { transport });
    await expect(client.get("spaces", { name: "pm10" })).rejects.toThrow(/keyed by \(project,name\)/);
    expect(sent).toHaveLength(0);
  });
});

describe("the generated types", () => {
  const csdl = JSON.parse(readFileSync(SNAPSHOT, "utf8"));

  it("are what the $metadata snapshot generates", () => {
    expect(readFileSync(TARGET, "utf8")).toBe(generate(csdl));
  });

  it("would not match after a hand edit", () => {
    const edited = readFileSync(TARGET, "utf8").replace("export interface Project {", "export interface Project {\n  extra: string;");
    expect(edited).not.toBe(generate(csdl));
  });
});
