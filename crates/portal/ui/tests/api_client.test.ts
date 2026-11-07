import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  isPublicFormPath,
  ApiError,
  createSessionMiddleware,
  csrfMiddleware,
  loginRedirectUrl,
  queryKeys,
  readCsrfToken,
  unwrap,
} from "../src/api/client";
import type { ProblemDetails } from "../src/api/client";
import { searchEntities } from "../src/api/gateway";
import { redirectTarget } from "../src/routes/LoginPage";

describe("readCsrfToken", () => {
  beforeEach(() => {
    // Clear cookies in jsdom
    for (const cookie of document.cookie.split(";")) {
      const eqIdx = cookie.indexOf("=");
      const name = eqIdx > -1 ? cookie.slice(0, eqIdx).trim() : cookie.trim();
      if (name) {
        document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; path=/`;
      }
    }
  });

  it("returns the value from document.cookie when multiple cookies are present", () => {
    document.cookie = "session_id=xyz123; path=/";
    document.cookie = "__Host-jc_csrf=test-csrf-token; Secure; path=/";
    document.cookie = "pref=dark; path=/";

    expect(readCsrfToken()).toBe("test-csrf-token");
  });

  it("returns undefined when __Host-jc_csrf cookie is absent", () => {
    document.cookie = "session_id=xyz123; path=/";
    document.cookie = "pref=dark; path=/";

    expect(readCsrfToken()).toBeUndefined();
  });

  it("decodes URI encoded cookie value", () => {
    document.cookie = `__Host-jc_csrf=${encodeURIComponent("token/with+special=chars")}; Secure; path=/`;

    expect(readCsrfToken()).toBe("token/with+special=chars");
  });
});

describe("csrfMiddleware", () => {
  beforeEach(() => {
    for (const cookie of document.cookie.split(";")) {
      const eqIdx = cookie.indexOf("=");
      const name = eqIdx > -1 ? cookie.slice(0, eqIdx).trim() : cookie.trim();
      if (name) {
        document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; path=/`;
      }
    }
  });

  async function run(method: string): Promise<Request> {
    const request = new Request(`${window.location.origin}/api/v1/health`, { method });
    const onRequest = csrfMiddleware.onRequest;
    if (!onRequest) {
      throw new Error("csrfMiddleware has no onRequest hook");
    }
    const result = await onRequest({ request } as Parameters<typeof onRequest>[0]);
    return result instanceof Request ? result : request;
  }

  it("does not add x-csrf-token to a GET request", async () => {
    document.cookie = "__Host-jc_csrf=my-token; Secure; path=/";

    const request = await run("GET");
    expect(request.headers.get("x-csrf-token")).toBeNull();
  });

  it("adds x-csrf-token to a POST request when the __Host-jc_csrf cookie is present", async () => {
    document.cookie = "__Host-jc_csrf=my-token; Secure; path=/";

    const request = await run("POST");
    expect(request.headers.get("x-csrf-token")).toBe("my-token");
  });

  it("leaves a POST unchanged when the __Host-jc_csrf cookie is absent", async () => {
    const request = await run("POST");
    expect(request.headers.get("x-csrf-token")).toBeNull();
  });

  it("covers every unsafe method", async () => {
    document.cookie = "__Host-jc_csrf=my-token; Secure; path=/";

    for (const method of ["POST", "PUT", "PATCH", "DELETE"]) {
      const request = await run(method);
      expect(request.headers.get("x-csrf-token"), method).toBe("my-token");
    }
    for (const method of ["GET", "HEAD", "OPTIONS"]) {
      const request = await run(method);
      expect(request.headers.get("x-csrf-token"), method).toBeNull();
    }
  });
});

describe("sessionMiddleware", () => {
  async function answer(path: string, status: number, navigate: (url: string) => void) {
    const middleware = createSessionMiddleware(navigate);
    const onResponse = middleware.onResponse;
    if (!onResponse) {
      throw new Error("sessionMiddleware has no onResponse hook");
    }
    const request = new Request(`${window.location.origin}${path}`);
    const response = new Response(null, { status });
    const result = await onResponse({ request, response } as Parameters<typeof onResponse>[0]);
    return result instanceof Response ? result : response;
  }

  it("sends a 401 on a list call to the login page with the current location", async () => {
    window.history.replaceState(null, "", "/projects/helsinki/spaces?page=2");
    const navigate = vi.fn();

    const response = await answer("/api/v1/projects/helsinki/spaces", 401, navigate);

    expect(navigate).toHaveBeenCalledWith(
      "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces%3Fpage%3D2"
    );
    expect(response.status).toBe(401);
  });

  it("leaves a 403 and a 200 alone so the page shows its own state", async () => {
    window.history.replaceState(null, "", "/projects/helsinki/spaces");
    const navigate = vi.fn();

    await answer("/api/v1/projects/helsinki/spaces", 403, navigate);
    await answer("/api/v1/projects/helsinki/spaces", 200, navigate);

    expect(navigate).not.toHaveBeenCalled();
  });

  it("treats a 401 from /auth/me as the anonymous answer, not a redirect", async () => {
    window.history.replaceState(null, "", "/projects/helsinki/spaces");
    const navigate = vi.fn();

    await answer("/api/v1/auth/me", 401, navigate);

    expect(navigate).not.toHaveBeenCalled();
  });

  it("never loops from the login page itself", async () => {
    window.history.replaceState(null, "", "/login?redirect_to=%2F");
    const navigate = vi.fn();

    await answer("/api/v1/projects/helsinki/spaces", 401, navigate);

    expect(navigate).not.toHaveBeenCalled();
  });

  it("the login page returns to the interrupted location and to same-origin paths only", () => {
    expect(loginRedirectUrl("/a?b=1")).toBe("/login?redirect_to=%2Fa%3Fb%3D1");
    expect(redirectTarget("?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces%3Fpage%3D2")).toBe(
      "/projects/helsinki/spaces?page=2"
    );
    expect(redirectTarget("?redirect_to=%2F%2Fevil.example")).toBe("/");
    expect(redirectTarget("?redirect_to=https%3A%2F%2Fevil.example")).toBe("/");
    expect(redirectTarget("")).toBe("/");
  });
});

describe("unwrap", () => {
  it("returns data on success", async () => {
    const mockData = { name: "test-resource" };
    const response = new Response(JSON.stringify(mockData), { status: 200 });

    const result = await unwrap({ data: mockData, response });
    expect(result).toEqual(mockData);
  });

  it("throws ApiError carrying status and detail on an RFC 7807 problem body", async () => {
    const problem: ProblemDetails = {
      type: "https://example.com/errors/not-found",
      title: "Not Found",
      status: 404,
      detail: "The requested project does not exist.",
    };
    const response = new Response(JSON.stringify(problem), { status: 404 });

    await expect(unwrap({ error: problem, response })).rejects.toThrow(ApiError);

    try {
      await unwrap({ error: problem, response });
    } catch (err: unknown) {
      expect(err).toBeInstanceOf(ApiError);
      const apiError = err as ApiError;
      expect(apiError.status).toBe(404);
      expect(apiError.problem).toEqual(problem);
      expect(apiError.problem?.detail).toBe("The requested project does not exist.");
      expect(apiError.message).toBe("The requested project does not exist.");
    }
  });

  it("throws ApiError with fallback status text when problem details are omitted", async () => {
    const response = new Response(null, { status: 500, statusText: "Internal Server Error" });

    try {
      await unwrap({ response });
    } catch (err: unknown) {
      expect(err).toBeInstanceOf(ApiError);
      const apiError = err as ApiError;
      expect(apiError.status).toBe(500);
      expect(apiError.message).toBe("Internal Server Error");
    }
  });
});

describe("queryKeys", () => {
  it("returns stable query key tuples", () => {
    expect(queryKeys.session()).toEqual(["session"]);
    expect(queryKeys.list("default", "workspaces")).toEqual(["projects", "default", "workspaces"]);
    expect(queryKeys.resource("default", "workspaces", "ws-1")).toEqual([
      "projects",
      "default",
      "workspaces",
      "ws-1",
    ]);
  });
});

describe("searchEntities", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("builds correct URL and headers and maps entity properties", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify([
          {
            id: "urn:ngsi-ld:Sensor:01",
            type: "Sensor",
            name: { type: "Property", value: "Temp Sensor" },
          },
          {
            id: "urn:ngsi-ld:Sensor:02",
            type: "Sensor",
          },
        ]),
        { status: 200, headers: { "Content-Type": "application/ld+json" } }
      )
    );

    const entities = await searchEntities({
      space: "city-iot",
      type: "Sensor",
      q: "Temp",
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(decodeURIComponent(url)).toContain(
      '/cs/city-iot/ngsi-ld/v1/entities?type=Sensor&limit=20&q=name~="Temp"'
    );
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>)?.Accept).toBe("application/ld+json");

    expect(entities).toEqual([
      { id: "urn:ngsi-ld:Sensor:01", type: "Sensor", name: "Temp Sensor" },
      { id: "urn:ngsi-ld:Sensor:02", type: "Sensor", name: undefined },
    ]);
  });

  it("searches the typed text as a literal, never as a pattern", async () => {
    // F-UI1-020: `~=` is a regular expression; a typed `a.*(b+)+` must reach the broker escaped.
    fetchMock.mockResolvedValue(new Response("[]", { status: 200, headers: { "Content-Type": "application/ld+json" } }));
    await searchEntities({ space: "city-iot", type: "Sensor", q: 'a.*(b+)+ "x"' });
    const [url] = fetchMock.mock.calls[0] as [string];
    expect(new URL(url, "https://portal.invalid").searchParams.get("q")).toBe(
      String.raw`name~="a\\.\\*\\(b\\+\\)\\+ \"x\""`,
    );
  });

  it("throws ApiError on non-2xx response", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          type: "about:blank",
          title: "Forbidden",
          status: 403,
          detail: "Access denied to space",
        }),
        { status: 403, headers: { "Content-Type": "application/problem+json" } }
      )
    );

    await expect(
      searchEntities({
        space: "restricted",
        type: "Sensor",
      })
    ).rejects.toThrow(ApiError);
  });

  it("propagates AbortError untouched when aborted", async () => {
    const controller = new AbortController();
    controller.abort();

    fetchMock.mockRejectedValue(new DOMException("The operation was aborted.", "AbortError"));

    await expect(
      searchEntities({
        space: "space1",
        type: "Sensor",
        signal: controller.signal,
      })
    ).rejects.toMatchObject({ name: "AbortError" });
  });
});

describe("unwrap", () => {
  it("returns the body of a successful answer", async () => {
    const response = new Response("{}", { status: 200 });
    await expect(unwrap({ data: { id: "r-1" }, response })).resolves.toEqual({ id: "r-1" });
  });

  it("treats 204 No Content as success, not as an error", async () => {
    // Answering the agent's question and sending it an instruction both answer 204. Reading
    // the empty body as a failure put "No Content" on the run page in red.
    const response = new Response(null, { status: 204, statusText: "No Content" });
    await expect(unwrap<void>({ response })).resolves.toBeUndefined();
  });

  it("still throws the problem a failing call carries", async () => {
    const problem: ProblemDetails = {
      type: "https://joinedcontext.com/errors/conflict",
      title: "Conflict",
      status: 409,
      detail: "the run is already cancelled",
    };
    const response = new Response(JSON.stringify(problem), { status: 409 });
    await expect(unwrap({ error: problem, response })).rejects.toThrow(
      "the run is already cancelled",
    );
  });

  it("throws on a failing call that carries no problem document", async () => {
    const response = new Response(null, { status: 502, statusText: "Bad Gateway" });
    await expect(unwrap({ response })).rejects.toThrow("Bad Gateway");
  });
});

describe("refName", () => {
  it("reads both shapes of a jc-core reference (MF-07)", async () => {
    const { refName } = await import("../src/api/manifest");
    expect(refName("helsinki")).toBe("helsinki");
    expect(refName({ kind: "ContextSpace", name: "helsinki" })).toBe("helsinki");
    // What broke the generated App manifest: a reference nested inside a reference.
    expect(refName({ kind: "ContextSpace", name: { name: "helsinki" } })).toBe("");
    expect(refName(undefined)).toBe("");
  });
});

describe("a list the caller may not read (T-3147)", () => {
  const refused = (detail: string, status = 404) => ({
    error: { title: "Not Found", status, detail },
    response: new Response(null, { status }),
  });

  it("says in the person's language that the list is not open to them, not the API's words", async () => {
    const i18n = (await import("../src/i18n")).default;
    await i18n.changeLanguage("sk");
    await expect(unwrap(refused("plural 'roles' not found in project 'helsinki'"))).rejects.toThrow(
      "Tento zoznam vám v helsinki nie je prístupný.",
    );
    await i18n.changeLanguage("en");
    await expect(unwrap(refused("plural 'rolebindings' not found in project 'org'"))).rejects.toThrow(
      "This list is not open to you in org.",
    );
  });

  it("keeps every other refusal in the API's words", async () => {
    await expect(unwrap(refused("project 'x' not found"))).rejects.toThrow("project 'x' not found");
    await expect(unwrap(refused("plural 'roles' not found in project 'helsinki'", 403))).rejects.toThrow(
      "plural 'roles' not found in project 'helsinki'",
    );
  });
});

describe("isPublicFormPath (EP-102)", () => {
  it("is a public form's page alone, never another page under it", () => {
    for (const path of ["/f/zt4qm7ge2xdv6ksb3ncf5arw2y", "/f/abc/"]) expect(isPublicFormPath(path), path).toBe(true);
    for (const path of ["/", "/f/", "/f/abc/more", "/projects/city/f/abc", "/fx/abc", "/v/abc"]) expect(isPublicFormPath(path), path).toBe(false);
  });
});

describe("a public form's requests (EP-102)", () => {
  afterEach(() => {
    window.history.pushState({}, "", "/");
    vi.unstubAllGlobals();
  });

  it("omit credentials on /f/ alone, decided per request as the router moves", async () => {
    const { api } = await import("../src/api/client");
    const sent: Request[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((request: Request) => {
        sent.push(request);
        return Promise.resolve(new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }));
      }),
    );
    for (const path of ["/projects/city", "/f/abc", "/projects/city"]) {
      window.history.pushState({}, "", path);
      await api.GET("/api/v1/auth/me");
    }
    expect(sent.map((request) => request.credentials)).toEqual(["same-origin", "omit", "same-origin"]);
  });
});
