// @vitest-environment jsdom
// T-4912: an organization-shared App's function runs with the viewer's own token, so a data call
// the viewer may not make is refused by the Endpoint, and the page says "no access", never "failed".
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createClient, ProblemError } from "../src/sdk/client";
import type { JcConfig } from "../src/sdk/config";
import { answeringRefusals } from "../src/sdk/functions-runtime";
import type { FnHandler } from "../src/sdk/server";
import { Problem } from "../src/sdk/shell";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";
import { SDK_WORDS } from "../src/sdk/words";

afterEach(cleanup);

const CONFIG: JcConfig = { slug: "events", orgDomain: "bbsk.sk", space: "events", transport: "bridge", language: "en" };

/** What the gateway's Policy answers a caller without a grant on the space (GW6: no rule named). */
const POLICY_REFUSAL = { type: "https://joinedcontext.com/errors/forbidden", title: "Access Denied by Policy", status: 403 };

const answering =
  (status: number, body: unknown, seen: JcRequest[] = []): Transport =>
  async (req: JcRequest): Promise<JcResponse> => {
    seen.push(req);
    return { status, body };
  };

const upcoming: FnHandler = async (_request, ctx) => ({ body: await ctx.jc.entities.list("Event") });

const request = { method: "POST" as const, query: {}, body: null, user: { id: "sub-eva", roles: [] } };

describe("answeringRefusals", () => {
  it("answers the Endpoint's 403 as the function's own 403, so the page can say no access", async () => {
    const seen: JcRequest[] = [];
    const ctx = { jc: createClient(CONFIG, answering(403, POLICY_REFUSAL, seen)), log: () => {} };
    const answer = await answeringRefusals(upcoming)(request, ctx);
    expect(answer).toEqual({ status: 403, body: { type: POLICY_REFUSAL.type, title: POLICY_REFUSAL.title, status: 403 } });
    expect(seen.map((r) => r.path)).toEqual([expect.stringMatching(/^\/api\/endpoint\/events\/ngsi-ld\/v1\/entities\?/)]);
  });

  it("answers a 401 the same way and leaves an allowed answer as it is", async () => {
    const refused = { jc: createClient(CONFIG, answering(401, {})), log: () => {} };
    expect((await answeringRefusals(upcoming)(request, refused)).status).toBe(401);
    const granted = { jc: createClient(CONFIG, answering(200, [])), log: () => {} };
    expect(await answeringRefusals(upcoming)(request, granted)).toEqual({ body: [] });
  });

  it("keeps any other failure the function's own", async () => {
    const broken = { jc: createClient(CONFIG, answering(502, "<html>bad gateway</html>")), log: () => {} };
    await expect(answeringRefusals(upcoming)(request, broken)).rejects.toMatchObject({ status: 502 });
    const thrown: FnHandler = async () => {
      throw new TypeError("x is undefined");
    };
    await expect(answeringRefusals(thrown)(request, broken)).rejects.toBeInstanceOf(TypeError);
  });
});

describe("a refused function on the page", () => {
  it("shows the no-access sentence, not a failure and not the Policy's English", async () => {
    const page = createClient(CONFIG, answering(403, { type: POLICY_REFUSAL.type, title: POLICY_REFUSAL.title, status: 403 }));
    const error = await page.functions.call("upcoming").catch((err: unknown) => err);
    expect(error).toBeInstanceOf(ProblemError);
    render(<Problem error={error as ProblemError} onRetry={() => {}} />);
    const text = screen.getByRole("alert").textContent ?? "";
    expect(text).toBe(SDK_WORDS.en["problem.denied"]);
    expect(text).not.toContain(SDK_WORDS.en["problem.failed"]);
    expect(text).not.toMatch(/Access Denied/);
    expect(screen.queryByRole("button", { name: SDK_WORDS.en["state.retry"] })).toBeNull();
  });

  it("keeps the retry of a failure, which asking again can mend", () => {
    render(<Problem error={new ProblemError(502, {})} onRetry={() => {}} />);
    expect(screen.getByRole("button", { name: SDK_WORDS.en["state.retry"] })).toBeInTheDocument();
  });
});
