import { describe, expect, it } from "vitest";
import { createClient, isEndpointPath, ProblemError, ServiceRefusedError } from "../src/sdk/client";
import type { JcConfig } from "../src/sdk/config";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";

const CONFIG: JcConfig = {
  slug: "demo",
  orgDomain: "example.org",
  space: "demo",
  transport: "bridge",
  language: "fi",
};

describe("isEndpointPath validation", () => {
  it("accepts exactly the seven allowed endpoint path shapes", () => {
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Ax%3A1")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Ax%3A1/attrs")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/temporal/entities")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/schema/index.json")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/schema/v1/json-schema")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/schema/v42/json-schema")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/access")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&limit=10")).toBe(true);
  });

  // SDK-05: the SDK writes only through its own endpoint; every other path is refused.
  it("refuses unauthorized paths, path traversal, multiple slashes, backslashes, and other slugs", () => {
    expect(isEndpointPath("demo", "/api/endpoint/other/ngsi-ld/v1/entities")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/subscriptions")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities/../access")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo//ngsi-ld/v1/entities")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities\\attrs")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/access#section")).toBe(false);
  });
});

describe("DataClient entity operations", () => {
  it("entities.all pages 1000 at a time, stops on short page, and caps at 5000", async () => {
    const calls: JcRequest[] = [];
    const scriptedTransport: Transport = async (req: JcRequest): Promise<JcResponse> => {
      calls.push(req);
      const url = new URL(req.path, "http://localhost");
      const offset = Number(url.searchParams.get("offset")) || 0;
      const limit = Number(url.searchParams.get("limit")) || 1000;
      const count = offset >= 2000 ? 500 : limit; // short page at 2000
      const page = Array.from({ length: count }, (_, i) => ({
        id: `urn:ngsi-ld:T:${offset + i}`,
        type: "T",
        n: offset + i,
      }));
      return { status: 200, body: page };
    };

    const client = createClient(CONFIG, scriptedTransport);
    const rows = await client.entities.all("T", { limit: 5000 });

    expect(rows).toHaveLength(2500);
    expect(calls).toHaveLength(3); // 0..1000, 1000..2000, 2000..2500
  });

  it("entities.list sends keyValues, strips id and type from attrs, and uses config.language", async () => {
    let sentPath = "";
    const transport: Transport = async (req) => {
      sentPath = req.path;
      return {
        status: 200,
        body: [
          {
            id: "urn:1",
            type: "Station",
            name: { languageMap: { fi: "Kamppi", en: "Kamppi EN" } },
          },
        ],
      };
    };

    const client = createClient(CONFIG, transport);
    const rows = await client.entities.list("Station", { attrs: ["id", "type", "name"] });

    expect(sentPath).toContain("attrs=name");
    expect(sentPath).not.toContain("attrs=id");
    expect(sentPath).toContain("options=keyValues");
    expect(rows[0].name).toBe("Kamppi"); // picked languageMap.fi
  });

  it("throws ProblemError with detail when endpoint refuses (e.g. 403)", async () => {
    const transport: Transport = async () => ({
      status: 403,
      body: { title: "Forbidden", detail: "Role cannot read this entity" },
    });

    const client = createClient(CONFIG, transport);
    await expect(client.entities.list("Secret")).rejects.toThrowError(ProblemError);
    await expect(client.entities.list("Secret")).rejects.toMatchObject({
      status: 403,
      detail: "Role cannot read this entity",
    });
  });

  it("throws ProblemError when list response is not an array", async () => {
    const transport: Transport = async () => ({ status: 200, body: { not: "an array" } });
    const client = createClient(CONFIG, transport);
    await expect(client.entities.list("T")).rejects.toThrowError("The endpoint did not answer a list.");
  });

  // SDK-05: create mints urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId} from the served configuration.
  it("create mints URN, encodes GeoProperty and Property, drops nulls, and validates type/localId", async () => {
    let postedBody: unknown;
    const transport: Transport = async (req) => {
      postedBody = req.body;
      return { status: 201, body: null };
    };

    const client = createClient(CONFIG, transport);

    // Bad localId refused before sending
    await expect(client.entities.create("T", {}, "invalid id with spaces")).rejects.toThrow(ProblemError);
    // Bad type refused before sending
    await expect(client.entities.create("bad-type!", {})).rejects.toThrow(ProblemError);

    const id = await client.entities.create(
      "Station",
      {
        name: "Kamppi",
        bikes: 10,
        empty: null,
        location: { type: "Point", coordinates: [24.9, 60.1] },
      },
      "001",
    );

    expect(id).toBe("urn:ngsi-ld:Station:example.org:demo:001");
    expect(postedBody).toEqual({
      id: "urn:ngsi-ld:Station:example.org:demo:001",
      type: "Station",
      name: { type: "Property", value: "Kamppi" },
      bikes: { type: "Property", value: 10 },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.9, 60.1] } },
    });
  });

  it("update sends PATCH and empty patch sends nothing", async () => {
    const calls: JcRequest[] = [];
    const transport: Transport = async (req) => {
      calls.push(req);
      return { status: 204, body: null };
    };

    const client = createClient(CONFIG, transport);
    await client.entities.update("urn:ngsi-ld:T:1", {});
    expect(calls).toHaveLength(0); // empty patch skips request

    await client.entities.update("urn:ngsi-ld:T:1", { bikes: 5 });
    expect(calls).toHaveLength(1);
    expect(calls[0].method).toBe("PATCH");
    expect(calls[0].path).toBe("/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Angsi-ld%3AT%3A1/attrs");
    expect(calls[0].body).toEqual({ bikes: { type: "Property", value: 5 } });
  });

  // SDK-07: a LanguageProperty is written as one, every language of its map in the one PATCH.
  it("update writes a language map as a LanguageProperty", async () => {
    const calls: JcRequest[] = [];
    const client = createClient(CONFIG, async (req) => {
      calls.push(req);
      return { status: 204, body: null };
    });
    await client.entities.update("urn:ngsi-ld:T:1", { name: { languageMap: { fi: "Tietyö", en: "Road work" } }, bikes: 5 });
    expect(calls[0].body).toEqual({
      name: { type: "LanguageProperty", languageMap: { fi: "Tietyö", en: "Road work" } },
      bikes: { type: "Property", value: 5 },
    });
  });

  it.each([
    ["a keyValues LanguageProperty", { languageMap: { fi: "Tietyö", sv: "Vägarbete" } }, { fi: "Tietyö", sv: "Vägarbete" }],
    ["a bare language map", { fi: "Tietyö", n: 3 }, { fi: "Tietyö" }],
    ["a plain string, in the application's language", "Tietyö", { fi: "Tietyö" }],
    ["no such attribute", undefined, {}],
  ])("languages reads every language of %s", async (_, stored, expected) => {
    const calls: JcRequest[] = [];
    const client = createClient(CONFIG, async (req) => {
      calls.push(req);
      return { status: 200, body: { id: "urn:ngsi-ld:T:1", type: "T", ...(stored === undefined ? {} : { name: stored }) } };
    });
    expect(await client.entities.languages("urn:ngsi-ld:T:1", "name")).toEqual(expected);
    expect(calls[0].method).toBe("GET");
    expect(calls[0].path).toBe("/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Angsi-ld%3AT%3A1?options=keyValues&attrs=name");
  });

  it("languages throws the endpoint's refusal", async () => {
    const client = createClient(CONFIG, async () => ({ status: 403, body: { title: "Forbidden", detail: "no read" } }));
    await expect(client.entities.languages("urn:ngsi-ld:T:1", "name")).rejects.toBeInstanceOf(ProblemError);
  });

  it("remove sends DELETE to entity path", async () => {
    let deletedPath = "";
    const transport: Transport = async (req) => {
      deletedPath = req.path;
      return { status: 204, body: null };
    };
    const client = createClient(CONFIG, transport);
    await client.entities.remove("urn:1");
    expect(deletedPath).toBe("/api/endpoint/demo/ngsi-ld/v1/entities/urn%3A1");
  });
});

describe("temporal, schema, and functions", () => {
  it("temporal.list requires endTimeAt for between, sends temporalValues, and parses response", async () => {
    let reqPath = "";
    const transport: Transport = async (req) => {
      reqPath = req.path;
      return {
        status: 200,
        body: [
          {
            id: "urn:1",
            type: "T",
            speed: {
              type: "Property",
              values: [
                [10, "2026-09-12T10:00:00Z"],
                { value: 12, observedAt: "2026-09-12T11:00:00Z" },
              ],
            },
          },
        ],
      };
    };

    const client = createClient(CONFIG, transport);
    await expect(
      client.temporal.list("T", { timerel: "between", timeAt: "2026-09-12T00:00:00Z" }),
    ).rejects.toThrow("endTimeAt is required");

    const rows = await client.temporal.list("T", {
      timerel: "after",
      timeAt: "2026-09-12T00:00:00Z",
    });

    expect(reqPath).toContain("options=temporalValues");
    expect(rows).toHaveLength(1);
    expect(rows[0].series.speed).toEqual([
      { value: 10, observedAt: "2026-09-12T10:00:00Z" },
      { value: 12, observedAt: "2026-09-12T11:00:00Z" },
    ]);
  });

  it("temporal.list asks for the named entities only, and refuses an id that is no URN", async () => {
    let reqPath = "";
    const transport: Transport = async (req) => {
      reqPath = req.path;
      return { status: 200, body: [] };
    };
    const client = createClient(CONFIG, transport);
    await client.temporal.list("T", { timerel: "after", timeAt: "2026-09-12T00:00:00Z", id: ["urn:ngsi-ld:T:a", "urn:ngsi-ld:T:b"] });
    expect(new URLSearchParams(reqPath.slice(reqPath.indexOf("?") + 1)).get("id")).toBe("urn:ngsi-ld:T:a,urn:ngsi-ld:T:b");
    for (const bad of ["a", "urn:x,urn:y", "urn:x y", ""]) {
      await expect(client.temporal.list("T", { timerel: "after", timeAt: "2026-09-12T00:00:00Z", id: [bad] })).rejects.toThrow("Invalid entity id");
    }
  });

  it("schema resolves version from index.json, caches success, and retries on failure", async () => {
    let count = 0;
    const transport: Transport = async (req) => {
      if (req.path.endsWith("index.json")) {
        count++;
        if (count === 1) return { status: 500, body: { title: "Server error" } };
        return { status: 200, body: { models: [{ version: 2 }, { version: 3 }] } };
      }
      if (req.path.endsWith("/v2/json-schema")) return { status: 200, body: { definitions: { Station: { properties: {} } } } };
      return { status: 200, body: { $defs: { Bike: { properties: {} } } } };
    };

    const client = createClient(CONFIG, transport);
    await expect(client.schema()).rejects.toThrow();

    const s1 = await client.schema();
    expect(s1).toHaveProperty("Station");
    expect(s1).toHaveProperty("Bike");

    // Second call should hit the cache (no more requests)
    const s2 = await client.schema();
    expect(s2).toBe(s1);
  });

  it("schema lists only the types the index serves, never the abstract Entity base class", async () => {
    const transport: Transport = async (req) => {
      if (req.path.endsWith("index.json")) {
        return { status: 200, body: { models: [{ version: 1, types: ["BikeHireDockingStation"] }] } };
      }
      return {
        status: 200,
        body: { definitions: { BikeHireDockingStation: { properties: {} }, Entity: { properties: {} }, Other: { properties: {} } } },
      };
    };
    expect(Object.keys(await createClient(CONFIG, transport).schema())).toEqual(["BikeHireDockingStation"]);

    // An index that names no types still drops the base class.
    const untyped: Transport = async (req) =>
      req.path.endsWith("index.json")
        ? { status: 200, body: { models: [{ version: 1 }] } }
        : { status: 200, body: { definitions: { Station: { properties: {} }, Entity: { properties: {} } } } };
    expect(Object.keys(await createClient(CONFIG, untyped).schema())).toEqual(["Station"]);
  });

  it("functions.call routes to /functions/{name} on bridge and /api/functions/{name} on its own host", async () => {
    const calls: JcRequest[] = [];
    const transport: Transport = async (req) => {
      calls.push(req);
      if (req.path.includes("runtime-err")) {
        return {
          status: 500,
          body: { error: { message: "Division by zero", file: "math.ts", line: 42 } },
        };
      }
      return { status: 200, body: { result: "ok" } };
    };

    const bridgeClient = createClient({ ...CONFIG, transport: "bridge" }, transport);
    const originClient = createClient({ ...CONFIG, transport: "origin", appName: "my-app" }, transport);

    await expect(bridgeClient.functions.call("BadName!")).rejects.toThrow("Invalid function name");

    const r1 = await bridgeClient.functions.call("rebalance", { factor: 2 });
    expect(calls[0].path).toBe("/functions/rebalance");
    expect(r1).toEqual({ result: "ok" });

    await originClient.functions.call("notify", { to: "user" });
    expect(calls[1].path).toBe("/api/functions/notify");

    try {
      await bridgeClient.functions.call("runtime-err");
      expect.unreachable("should have thrown");
    } catch (err) {
      expect(err).toBeInstanceOf(ProblemError);
      const pe = err as ProblemError;
      expect(pe.message).toBe("Division by zero");
      expect(pe.file).toBe("math.ts");
      expect(pe.line).toBe(42);
    }
  });
});

describe("email", () => {
  it("sends on the App's own host and turns a switched-off service or a used-up quota into ServiceRefusedError", async () => {
    const calls: JcRequest[] = [];
    const answers = [
      { status: 202, body: { id: "m-1" } },
      { status: 403, body: { type: "https://joinedcontext.com/errors/service-off", title: "Service Off", status: 403, service: "email", layer: "project" } },
      { status: 429, body: { type: "https://joinedcontext.com/errors/quota", title: "Quota Used", status: 429, service: "email", quota: "emailsPerDay", resetAt: "2026-10-11T00:00:00Z" } },
      { status: 403, body: { type: "https://joinedcontext.com/errors/recipient-refused", title: "Recipient Refused", status: 403, to: "x" } },
    ];
    const transport: Transport = async (req) => {
      calls.push(req);
      return answers[calls.length - 1];
    };
    const client = createClient({ ...CONFIG, transport: "origin" }, transport);
    const message = { to: "me" as const, subject: "New road defect", text: "Hlavná 4" };

    expect(await client.email.send(message)).toEqual({ id: "m-1" });
    expect(calls[0]).toEqual({ method: "POST", path: "/api/services/email/send", body: message });

    const off = await client.email.send(message).catch((err: unknown) => err);
    expect(off).toBeInstanceOf(ServiceRefusedError);
    expect(off).toMatchObject({ status: 403, service: "email", layer: "project" });

    const quota = await client.email.send(message).catch((err: unknown) => err);
    expect(quota).toMatchObject({ status: 429, quota: "emailsPerDay", resetAt: "2026-10-11T00:00:00Z" });

    const refused = await client.email.send(message).catch((err: unknown) => err);
    expect(refused).toBeInstanceOf(ProblemError);
    expect(refused).not.toBeInstanceOf(ServiceRefusedError);
  });
});

describe("jobs", () => {
  it("lists the App's jobs on its own host and turns a switched-off service into ServiceRefusedError", async () => {
    const calls: JcRequest[] = [];
    const listed = [{ name: "report", schedule: "*/15 * * * *", nextRun: "2026-10-10T21:45:00Z", lastRun: { at: "2026-10-10T21:15:00Z", ok: false, message: "503" } }];
    const answers = [
      { status: 200, body: listed },
      { status: 403, body: { type: "https://joinedcontext.com/errors/service-off", title: "Service Off", status: 403, service: "jobs", layer: "organization" } },
    ];
    const transport: Transport = async (req) => {
      calls.push(req);
      return answers[calls.length - 1];
    };
    const client = createClient({ ...CONFIG, transport: "origin" }, transport);
    expect(await client.jobs.list()).toEqual(listed);
    expect(calls[0]).toEqual({ method: "GET", path: "/api/services/jobs" });
    const off = await client.jobs.list().catch((err: unknown) => err);
    expect(off).toBeInstanceOf(ServiceRefusedError);
    expect(off).toMatchObject({ service: "jobs", layer: "organization" });
  });
});

describe("files", () => {
  it("keeps objects on the App's own host, a key's segments encoded, and names the quota it ran out of", async () => {
    const calls: JcRequest[] = [];
    const info = { path: "defects/1 a.jpg", size: 4, contentType: "image/jpeg", modifiedAt: "2026-10-10T22:00:00Z" };
    const blob = new Blob(["jpeg"], { type: "image/jpeg" });
    const answers = [
      { status: 201, body: info },
      { status: 200, body: blob },
      { status: 200, body: [info] },
      { status: 204, body: null },
      { status: 200, body: { url: "https://files.dev.example/apps/x?X-Amz-Expires=300", expiresAt: "2026-10-10T22:05:00Z" } },
      { status: 429, body: { type: "https://joinedcontext.com/errors/quota", title: "Quota Used", status: 429, service: "files", quota: "filesMiB" } },
    ];
    const transport: Transport = async (req) => {
      calls.push(req);
      return answers[calls.length - 1];
    };
    const client = createClient({ ...CONFIG, transport: "origin" }, transport);

    expect(await client.files.put("defects/1 a.jpg", blob)).toEqual(info);
    expect(calls[0]).toEqual({
      method: "PUT",
      path: "/api/services/files/defects/1%20a.jpg",
      raw: { data: blob, contentType: "image/jpeg" },
    });
    expect(await client.files.get("defects/1 a.jpg")).toBe(blob);
    expect(calls[1]).toMatchObject({ method: "GET", as: "blob" });
    expect(await client.files.list("defects/")).toEqual([info]);
    expect(calls[2].path).toBe("/api/services/files?prefix=defects%2F");
    await client.files.remove("defects/1 a.jpg");
    expect(calls[3].method).toBe("DELETE");
    expect((await client.files.url("defects/1 a.jpg", { method: "PUT", size: 2048 })).expiresAt).toBe("2026-10-10T22:05:00Z");
    expect(calls[4]).toEqual({ method: "POST", path: "/api/services/files/defects/1%20a.jpg:url", body: { method: "PUT", size: 2048 } });
    const full = await client.files.put("notes.txt", "x").catch((err: unknown) => err);
    expect(full).toBeInstanceOf(ServiceRefusedError);
    expect(full).toMatchObject({ service: "files", quota: "filesMiB" });
    expect((calls[5].raw as { contentType: string }).contentType).toBe("text/plain;charset=utf-8");
  });

  it("refuses a file key that climbs out of the files service before anything is sent (T-6230)", async () => {
    const calls: JcRequest[] = [];
    const transport: Transport = async (req) => {
      calls.push(req);
      return { status: 204, body: null };
    };
    const client = createClient({ ...CONFIG, transport: "origin" }, transport);
    for (const key of ["../../endpoint/x/ngsi-ld/v1/entities/u", "a/./b", "a//b", "/a", ".."]) {
      await expect(client.files.remove(key)).rejects.toBeInstanceOf(ProblemError);
      await expect(client.files.get(key)).rejects.toThrow("segment");
    }
    expect(calls).toEqual([]);
    // A dot inside a name is a name.
    await client.files.remove("reports/2026.10..csv");
    expect(calls[0].path).toBe("/api/services/files/reports/2026.10..csv");
  });
});

describe("ai", () => {
  it("asks the platform's model on the App's own host and names the day's budget it ran out of", async () => {
    const calls: JcRequest[] = [];
    const answers = [
      { status: 200, body: { text: "{\"severity\":3}", json: { severity: 3 }, tokens: { in: 70, out: 30 } } },
      { status: 429, body: { type: "https://joinedcontext.com/errors/quota", title: "Quota Used", status: 429, service: "ai", quota: "aiTokensPerDay", resetAt: "2026-10-11T00:00:00Z" } },
    ];
    const transport: Transport = async (req) => {
      calls.push(req);
      return answers[calls.length - 1];
    };
    const client = createClient({ ...CONFIG, transport: "origin" }, transport);
    const request = { messages: [{ role: "user" as const, content: "How bad?" }], schema: { type: "object" } };
    expect((await client.ai.complete(request)).json).toEqual({ severity: 3 });
    expect(calls[0]).toEqual({ method: "POST", path: "/api/services/ai/complete", body: request });
    const spent = await client.ai.complete(request).catch((err: unknown) => err);
    expect(spent).toBeInstanceOf(ServiceRefusedError);
    expect(spent).toMatchObject({ service: "ai", quota: "aiTokensPerDay", resetAt: "2026-10-11T00:00:00Z" });
  });
});
