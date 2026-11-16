import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/api/client";
import { act, call, door, doorAction, projectBound } from "../src/api/odata";
import { announceSessionEnded } from "../src/api/sessionEnded";
import { announceStepUp, stepUpUrl } from "../src/api/stepUp";
import { odataCall, odataError, odataValue } from "./odata";

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
    await act("approve", "changes(project='air',name='42')", {}, 'W/"1"').catch(
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
