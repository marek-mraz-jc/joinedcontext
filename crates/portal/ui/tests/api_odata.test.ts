import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/api/client";
import {
  act,
  approveChange,
  call,
  door,
  doorAction,
  listManifests,
  listOrganizations,
  manifestEtag,
  projectBound,
  proposeManifest,
  readChange,
  readManifest,
  readRun,
  rejectChange,
  listRuns,
} from "../src/api/odata";
import { importBody } from "../src/api/proposal";
import { announceSessionEnded } from "../src/api/sessionEnded";
import { announceStepUp, stepUpUrl } from "../src/api/stepUp";
import { manifestAsRest, odataCall, odataError, odataValue } from "./odata";

vi.mock("../src/api/sessionEnded", async (actual) => ({
  ...(await actual<object>()),
  announceSessionEnded: vi.fn(),
}));
vi.mock("../src/api/stepUp", async (actual) => ({
  ...(await actual<object>()),
  announceStepUp: vi.fn(),
}));

/** The requests the client sent, and `answer` for each. */
function serve(answer: (request: Request) => Response): Request[] {
  const sent: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      sent.push(request);
      return answer(request);
    }),
  );
  return sent;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
  document.cookie =
    "__Host-jc_csrf=; expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; path=/";
});

describe("the Portal's OData client (T-6812)", () => {
  it("calls a bound function of the project with its arguments as aliases and reads its value", async () => {
    const sent = serve(() => odataValue({ items: [], next: null }));
    await expect(
      call("jc_change_history", "air's", { kind: "Policy", name: "a/b" }),
    ).resolves.toEqual({ items: [], next: null });
    expect(sent).toHaveLength(1);
    expect(sent[0].method).toBe("GET");
    expect(sent[0].headers.get("OData-Version")).toBe("4.01");
    expect(odataCall(sent[0].url)).toEqual({
      project: "air's",
      name: "jc_change_history",
      params: { kind: "Policy", name: "a/b" },
    });
  });

  it("posts an action with its If-Match and the session's CSRF token", async () => {
    document.cookie = "__Host-jc_csrf=t0ken; Secure; path=/";
    const sent = serve(() => odataValue({ kind: "Change" }));
    await act(
      "propose_delete",
      `${projectBound("air")}/policies('p')`,
      { dryRun: true },
      'W/"7"',
    );
    expect(sent[0].method).toBe("POST");
    expect(new URL(sent[0].url).pathname).toBe(
      "/api/v1/projects('air')/policies('p')/JoinedContext.Portal.propose_delete",
    );
    expect(sent[0].headers.get("If-Match")).toBe('W/"7"');
    expect(sent[0].headers.get("x-csrf-token")).toBe("t0ken");
    await expect(sent[0].json()).resolves.toEqual({ dryRun: true });
  });

  it("reads a refusal as the ApiError every page shows, with the problem's members and the edge's reference", async () => {
    serve(() => {
      const refusal = odataError(
        403,
        "forbidden",
        "You may not read this project's changes.",
      );
      refusal.headers.set("X-Request-Id", "req-42");
      return refusal;
    });
    const refused = await call("jc_change_history", "air", {}).catch(
      (error: unknown) => error,
    );
    expect(refused).toBeInstanceOf(ApiError);
    expect(refused).toMatchObject({
      status: 403,
      requestId: "req-42",
      message: "You may not read this project's changes.",
      problem: {
        type: "https://joinedcontext.com/errors/forbidden",
        status: 403,
        detail: "You may not read this project's changes.",
      },
    });
  });

  it("calls a Portal door on the Organization or a project, reads a 204's null, and keeps each field a refusal names", async () => {
    const sent = serve((request) =>
      request.url.includes("portal_secret_set")
        ? odataError(
            422,
            "unprocessable",
            "the secret cannot be written as sent",
            {
              target: "values.apiKey",
              details: [
                {
                  code: "unprocessable",
                  target: "values.apiKey",
                  message: "1 to 16384 bytes",
                },
              ],
              hint: "Correct each member errors names.",
            },
          )
        : odataValue(request.method === "GET" ? { items: [] } : null),
    );
    await expect(door("portal_secret_list", {}, "air")).resolves.toEqual({
      items: [],
    });
    await expect(
      doorAction("portal_person_remove_second_factor", { person: "sub-ada" }),
    ).resolves.toBeNull();
    expect(new URL(sent[0].url).pathname).toBe(
      "/api/v1/projects('air')/JoinedContext.Portal.portal_secret_list()",
    );
    expect(new URL(sent[1].url).pathname).toBe(
      "/api/v1/Organization/JoinedContext.Portal.portal_person_remove_second_factor",
    );
    await expect(sent[1].json()).resolves.toEqual({ person: "sub-ada" });
    const refused = await doorAction(
      "portal_secret_set",
      { body: { name: "x" } },
      "air",
    ).catch((error: unknown) => error);
    expect(refused).toMatchObject({
      status: 422,
      problem: {
        errors: ["values.apiKey: 1 to 16384 bytes"],
        hint: "Correct each member errors names.",
      },
    });
  });

  it("sends the Apps pages' doors to their own addresses: an App's name as a parameter, the templates on the Organization", async () => {
    const sent: Request[] = [];
    // Not vi.stubGlobal: tests/setup.ts would answer these doors from their REST stubs.
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      sent.push(request);
      return odataValue(request.method === "GET" ? { builds: [] } : { url: "https://forge/mr/1" });
    });
    await door("portal_app_builds", { name: "air/../x" }, "air");
    await doorAction("portal_app_restore", { name: "air-map", body: { commit: "abc1234" } }, "air");
    await door("portal_app_templates", {});
    const builds = new URL(sent[0].url);
    expect(builds.pathname).toBe("/api/v1/projects('air')/JoinedContext.Portal.portal_app_builds(name=@name)");
    // The name stays one quoted parameter: a `/` in it never reaches another address.
    expect(builds.searchParams.get("@name")).toBe("'air/../x'");
    expect(new URL(sent[1].url).pathname).toBe("/api/v1/projects('air')/JoinedContext.Portal.portal_app_restore");
    await expect(sent[1].json()).resolves.toEqual({ name: "air-map", body: { commit: "abc1234" } });
    expect(new URL(sent[2].url).pathname).toBe("/api/v1/Organization/JoinedContext.Portal.portal_app_templates()");
  });

  it("imports manifests through the project's jc_project_import with its If-Match, a check as dryRun (T-7143)", async () => {
    const sent: Request[] = [];
    // Not vi.stubGlobal: tests/setup.ts would answer the action from the REST import's stub.
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      sent.push(request);
      return request.method === "GET"
        ? new Response(JSON.stringify({ "@odata.etag": 'W/"p1"', name: "air" }), {
            headers: { "Content-Type": "application/json" },
          })
        : odataValue({ created: ["air"] });
    });
    const space = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "air" }, spec: {} };
    await expect(
      importBody("air", { manifests: [space], conflictPolicy: "fail", spaceMapping: [] }, true),
    ).resolves.toEqual({ created: ["air"] });
    expect(new URL(sent[0].url).pathname).toBe("/api/v1/projects('air')");
    expect(sent[1].method).toBe("POST");
    expect(new URL(sent[1].url).pathname).toBe("/api/v1/projects('air')/JoinedContext.Portal.jc_project_import");
    expect(sent[1].headers.get("If-Match")).toBe('W/"p1"');
    // An empty spaceMapping is left out; the REST door's `?dryRun=All` is the action's `dryRun`.
    await expect(sent[1].json()).resolves.toEqual({ manifests: [space], dryRun: true, conflictPolicy: "fail" });
  });

  it("reads a Change by its key with every member of the REST door, and decides it with its actions or the org namespace's doors (T-7122)", async () => {
    const sent: Request[] = [];
    // Not vi.stubGlobal: tests/setup.ts would answer these from their REST stubs.
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      sent.push(request);
      const change = { metadata: { name: "chg-1" }, status: { phase: "Merged" } };
      return request.method === "GET" && request.url.includes("changes(")
        ? new Response(JSON.stringify({ "@odata.etag": 'W/"1"', name: "chg-1", ...change }), {
            headers: { "Content-Type": "application/json" },
          })
        : odataValue(change);
    });
    const read = await readChange("air's", "chg/../1");
    expect(read.status.phase).toBe("Merged");
    const key = new URL(sent[0].url);
    // The id stays one quoted key member: a `/` in it never reaches another address.
    expect(decodeURIComponent(key.pathname)).toBe("/api/v1/changes(project='air''s',name='chg/../1')");
    expect(key.searchParams.get("$select")?.split(",")).toEqual(
      expect.arrayContaining(["metadata", "status", "summary", "files", "planFields", "workspace", "ci", "fileCount", "decision"]),
    );
    await approveChange("air", "chg-1", "mobility");
    await rejectChange("air", "chg-1", "too wide");
    expect(decodeURIComponent(new URL(sent[1].url).pathname)).toBe("/api/v1/changes(project='air',name='chg-1')/JoinedContext.Portal.approve");
    // As the REST door's approve, which took no If-Match: the handler refuses a decided Change.
    expect(sent[1].headers.get("If-Match")).toBe("*");
    await expect(sent[1].json()).resolves.toEqual({ confirm: "mobility" });
    expect(decodeURIComponent(new URL(sent[2].url).pathname)).toBe("/api/v1/changes(project='air',name='chg-1')/JoinedContext.Portal.reject");
    await expect(sent[2].json()).resolves.toEqual({ reason: "too wide" });
    // The org namespace's own Changes are no row of the set: their Portal doors.
    await readChange("org", "chg-2");
    await approveChange("org", "chg-2");
    await rejectChange("org", "chg-2", "no");
    expect(new URL(sent[3].url).pathname).toBe("/api/v1/Organization/JoinedContext.Portal.portal_organization_change_get(id=@id)");
    expect(new URL(sent[4].url).pathname).toBe("/api/v1/Organization/JoinedContext.Portal.portal_organization_change_approve");
    await expect(sent[4].json()).resolves.toEqual({ id: "chg-2", body: {} });
    await expect(sent[5].json()).resolves.toEqual({ id: "chg-2", body: { reason: "no" } });
  });

  it("reads a kind's manifests through the project's navigation, the org namespace's from its Organization set, page after page", async () => {
    const sent: Request[] = [];
    // Not vi.stubGlobal: tests/setup.ts would answer these reads from REST stubs.
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      sent.push(request);
      const row = (name: string) => ({
        "@odata.etag": `W/"${name}"`,
        name,
        kind: "Policy",
        metadata: { name, namespace: "air" },
        spec: { rules: [] },
        status: null,
      });
      return new Response(
        JSON.stringify(
          request.url.includes("/policies?") &&
            !request.url.includes("skiptoken")
            ? {
                value: [row("a")],
                "@odata.nextLink": `${location.origin}/api/v1/projects('air''s')/policies?$skiptoken=1`,
              }
            : { value: [row("b")] },
        ),
        { headers: { "Content-Type": "application/json" } },
      );
    });
    const { items } = await listManifests("air's", "policies");
    expect(items).toEqual([
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Policy",
        metadata: { name: "a", namespace: "air" },
        spec: { rules: [] },
        status: null,
      },
      expect.objectContaining({ metadata: { name: "b", namespace: "air" } }),
    ]);
    expect(Object.keys(items[0])).not.toContain("@odata.etag");
    expect(manifestEtag(items[1])).toBe('W/"b"');
    const first = new URL(sent[0].url);
    expect(decodeURIComponent(first.pathname)).toBe(
      "/api/v1/projects('air''s')/policies",
    );
    expect(first.searchParams.get("$select")).toBe(
      "name,kind,metadata,spec,status,apiVersion",
    );
    expect(sent[1].url).toContain("$skiptoken=1");
    // The test's REST mapping reads the same address back to the REST door.
    expect(manifestAsRest(sent[0].url)?.rest.pathname).toBe(
      "/api/v1/projects/air's/policies",
    );

    sent.length = 0;
    await listManifests("org", "roles");
    await listManifests("org", "rolebindings");
    expect(sent.map((r) => new URL(r.url).pathname)).toEqual([
      "/api/v1/organizationRoles",
      "/api/v1/rolebindings",
    ]);
    vi.restoreAllMocks();
  });

  it("reads a project's runs and one run as the REST doors list and read them (T-7134)", async () => {
    // The page test setup reads the service's address back to the REST door whose handler
    // answers it (tests/odata.ts manifestAsRest): the stub sees the REST read.
    const run = { id: "r1", project: "air", status: "previewing", prompt: "bikes", dataNeeds: [] };
    const sent = serve((request) =>
      Response.json(request.url.endsWith("/agent-runs/r1") ? run : { items: [run] }),
    );
    const { items } = await listRuns("air", { kind: "conversation", createdBy: "o'neil", top: 20 });
    expect(items).toEqual([run]);
    const list = new URL(sent[0].url);
    expect(list.pathname).toBe("/api/v1/projects/air/agent-runs");
    expect(Object.fromEntries(list.searchParams)).toEqual({ kind: "conversation", mine: "true", limit: "20" });
    expect(await readRun("air", "r1")).toEqual(run);
    expect(new URL(sent[1].url).pathname).toBe("/api/v1/projects/air/agent-runs/r1");
    // The addresses the service is asked: a quote doubled inside the filter's literal.
    const odata = new URL(
      "/api/v1/projects('air')/runs?$filter=" +
        encodeURIComponent("appName eq 'o''neil'") +
        "&$select=id,prompt",
      "http://localhost",
    );
    expect(Object.fromEntries(manifestAsRest(odata)?.rest.searchParams ?? [])).toEqual({ origin: "all", app: "o'neil" });
    vi.restoreAllMocks();
  });

  it("lists people's runs unless the journeys' test runs are asked for, by the run's origin (T-7145)", async () => {
    // The set holds every origin: people's runs only are `origin eq 'person'` in its `$filter`,
    // the REST list's default; none there is its `origin=all` (tests/odata.ts manifestAsRest).
    const sent = serve(() => Response.json({ items: [] }));
    await listRuns("air", { kind: "application" });
    await listRuns("air", { kind: "application", origin: "all" });
    expect(sent.map((request) => Object.fromEntries(new URL(request.url).searchParams))).toEqual([
      { kind: "application" },
      { origin: "all", kind: "application" },
    ]);
    const journeys = new URL("/api/v1/projects('air')/runs?$filter=" + encodeURIComponent("origin eq 'journey'"), "http://localhost");
    expect(Object.fromEntries(manifestAsRest(journeys)?.rest.searchParams ?? [])).toEqual({ origin: "journey" });
    vi.unstubAllGlobals();
  });

  it("asks no project for an Organization set: a project holds none of its manifests (T-7118)", async () => {
    // The service has no `projects('helsinki')/groups` navigation and answers it 400; the REST
    // door listed the project's namespace, where an Organization kind is never stored.
    const sent = serve(() => odataValue([]));
    await expect(listManifests("helsinki", "groups")).resolves.toEqual({ items: [] });
    const missing = (await readManifest("helsinki", "rolebindings", "admins").catch(
      (e: unknown) => e,
    )) as ApiError;
    expect([missing.status, missing.message]).toEqual([
      404,
      "Your roles do not let you read the settings of admins, or it no longer exists.",
    ]);
    const update = (await proposeManifest("helsinki", "groups", { metadata: { name: "admins" } }, { create: false }).catch(
      (e: unknown) => e,
    )) as ApiError;
    expect(update.status).toBe(404);
    expect(sent).toEqual([]);
  });

  it("reads one manifest by its key with its ETag, and a missing one as the ApiError a page shows", async () => {
    const sent: Request[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const request = input as Request;
      sent.push(request);
      return request.url.includes("gone")
        ? odataError(404, "resource-not-found", "no endpoint gone you may read")
        : new Response(
            JSON.stringify({
              "@odata.etag": 'W/"7"',
              name: "bikes",
              kind: "Endpoint",
              metadata: { name: "bikes", namespace: "air" },
              spec: { slug: "bikes" },
              status: { phase: "Live" },
            }),
            { headers: { "Content-Type": "application/json" } },
          );
    });
    const endpoint = await readManifest("air", "endpoints", "bikes");
    expect(endpoint).toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "bikes", namespace: "air" },
      spec: { slug: "bikes" },
      status: { phase: "Live" },
    });
    expect(manifestEtag(endpoint)).toBe('W/"7"');
    expect(decodeURIComponent(new URL(sent[0].url).pathname)).toBe(
      "/api/v1/endpoints(project='air',name='bikes')",
    );
    await readManifest("org", "groups", "stewards");
    expect(decodeURIComponent(new URL(sent[1].url).pathname)).toBe(
      "/api/v1/groups('stewards')",
    );
    const missing = (await readManifest("air", "endpoints", "gone").catch(
      (e: unknown) => e,
    )) as ApiError;
    expect(missing).toBeInstanceOf(ApiError);
    expect([missing.status, missing.message]).toEqual([
      404,
      "no endpoint gone you may read",
    ]);
    vi.restoreAllMocks();
  });

  it("reads the Projects and the Organization's own manifest, none where the caller reads only the row (T-7104)", async () => {
    const sent: string[] = [];
    let readsOrganization = true;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = new URL((input as Request).url);
      sent.push(decodeURIComponent(url.pathname + url.search));
      const project = (name: string, readable: boolean) => ({
        "@odata.etag": `W/"${name}"`,
        name,
        metadata: readable ? { name, namespace: "org", title: "Air" } : null,
        spec: readable ? { quota: {} } : null,
        status: null,
      });
      const body = url.pathname.endsWith("/Organization")
        ? !readsOrganization
          ? { name: "hel", metadata: null, spec: null, status: null }
          : { "@odata.etag": 'W/"o"', name: "hel", metadata: { name: "hel" }, spec: { domain: "hel.fi" }, status: null }
        : url.pathname.includes("('noise')")
          ? project("noise", false)
          : url.pathname.includes("('air')")
            ? project("air", true)
            : { value: [project("air", true), project("noise", false)] };
      return new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
    });
    const { items } = await listManifests("org", "projects");
    expect(items.map((p) => [p.kind, p.metadata.name])).toEqual([["Project", "air"]]);
    const air = await readManifest("org", "projects", "air");
    expect([air.kind, manifestEtag(air)]).toEqual(["Project", 'W/"air"']);
    const hidden = (await readManifest("org", "projects", "noise").catch((e: unknown) => e)) as ApiError;
    expect([hidden instanceof ApiError, hidden.status]).toEqual([true, 404]);
    const organizations = await listOrganizations();
    expect(organizations.items).toEqual([
      { apiVersion: "joinedcontext.com/v1alpha1", kind: "Organization", metadata: { name: "hel" }, spec: { domain: "hel.fi" }, status: null },
    ]);
    expect(manifestEtag(organizations.items[0])).toBe('W/"o"');
    readsOrganization = false;
    expect((await listOrganizations()).items).toEqual([]);
    expect(sent).toEqual([
      "/api/v1/projects?$select=name,metadata,spec,status",
      "/api/v1/projects('air')?$select=name,metadata,spec,status",
      "/api/v1/projects('noise')?$select=name,metadata,spec,status",
      "/api/v1/Organization?$select=name,spec,status,metadata",
      "/api/v1/Organization?$select=name,spec,status,metadata",
    ]);
    vi.restoreAllMocks();
  });

  it("reads the Organizations from the REST list on an address that serves several (T-7104)", async () => {
    const sent: string[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = new URL((input as Request).url);
      sent.push(url.pathname);
      return url.pathname === "/api/v1/Organization"
        ? odataError(404, "several-organizations", "this address serves several Organizations")
        : new Response(JSON.stringify({ items: [{ kind: "Organization", metadata: { name: "zilina" } }, { kind: "Organization", metadata: { name: "hel" } }] }), {
            headers: { "Content-Type": "application/json" },
          });
    });
    const { items } = await listOrganizations();
    expect(items.map((o) => o.metadata.name)).toEqual(["zilina", "hel"]);
    expect(sent).toEqual(["/api/v1/Organization", "/api/v1/projects/org/organizations"]);
    // A person who sees several Organizations reads the list, never the one the service serves.
    expect((await listOrganizations(true)).items).toHaveLength(2);
    expect(sent.slice(2)).toEqual(["/api/v1/projects/org/organizations"]);
    vi.restoreAllMocks();
  });

  it("ends the session on a 401 and asks for a second factor on that refusal, as the REST client does", async () => {
    window.history.pushState({}, "", "/projects/air/approvals?tab=history");
    serve(() => new Response(null, { status: 401 }));
    await call("jc_change_history", "air", {}).catch(() => undefined);
    expect(announceSessionEnded).toHaveBeenCalledWith(
      `/login?redirect_to=${encodeURIComponent("/projects/air/approvals?tab=history")}`,
    );

    serve(() =>
      odataError(
        403,
        "second-factor-required",
        "Confirm with your second factor.",
      ),
    );
    // An action no REST door stands for in tests/setup.ts: a Change's approve reaches a stub as its REST door.
    await act("jc_change_apply", "projects('air')", { planId: "p1" }).catch(
      () => undefined,
    );
    expect(announceStepUp).toHaveBeenCalledWith(
      stepUpUrl("/projects/air/approvals?tab=history"),
    );

    vi.mocked(announceStepUp).mockClear();
    serve(() => odataError(403, "forbidden", "No."));
    await call("jc_change_history", "air", {}).catch(() => undefined);
    expect(announceStepUp).not.toHaveBeenCalled();
  });
});
