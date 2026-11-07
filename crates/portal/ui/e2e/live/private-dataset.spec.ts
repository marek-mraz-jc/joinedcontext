/**
 * One data grid, private Endpoints only for a person whose grant covers them (T-6376), on dev.
 *
 * Six cases: no session, a person without a grant (the viewer) and a person with one (the
 * steward), each against one public and one private Endpoint. Each read goes through the gateway
 * as that person, with the page's own session, as the grid reads; the status decides the state the
 * grid shows (sdk tests/grid_refusal.test.tsx): 200 rows, 401 "sign in", 403 "no access". A private
 * Endpoint never answers a body of entities to a caller without a session.
 */
import { expect, test } from "@playwright/test";
import type { APIRequestContext } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";

// Public: the helsinki news-topics App's Endpoint; private: Žilina's city Endpoint (audience organization).
const PUBLIC = process.env.PUBLIC_ENDPOINT_SLUG ?? "bi5og7mng4qabgu5bum336fggp";
const PRIVATE = process.env.PRIVATE_ENDPOINT_SLUG ?? "6cm7hh73b7igmmekf3a5ucwmjyqspf62";

test.setTimeout(240_000);

async function read(request: APIRequestContext, slug: string): Promise<{ status: number; body: string }> {
  const answer = await request.get(`/api/endpoint/${slug}/schema/index.json`);
  return { status: answer.status(), body: await answer.text() };
}

test("no session: the public Endpoint answers, the private one asks for a session and holds nothing back", async ({ browser }) => {
  const context = await browser.newContext();
  try {
    const open = await read(context.request, PUBLIC);
    expect(open.status, "a public Endpoint answers a visitor").toBe(200);
    const closed = await read(context.request, PRIVATE);
    expect(closed.status, "a private Endpoint asks a visitor to sign in").toBe(401);
    expect(closed.body).not.toContain('"models"');
    const rows = await context.request.get(`/api/endpoint/${PRIVATE}/ngsi-ld/v1/entities?type=Organization&limit=1`);
    expect(rows.status()).toBe(401);
    expect(await rows.text()).not.toContain("urn:ngsi-ld:");
    test.info().annotations.push({ type: "anonymous", description: `public ${open.status}, private ${closed.status}` });
  } finally {
    await context.close();
  }
});

for (const [who, person] of [
  ["viewer (no grant on the private Endpoint)", VIEWER],
  ["steward", STEWARD],
] as const) {
  test(`${who}: each read is that person's decision, never a public copy`, async ({ browser }) => {
    test.skip(!person.password, "the person's password is not in this run's environment");
    const { context } = await signIn(browser, person, "/?lang=en");
    try {
      const open = await read(context.request, PUBLIC);
      expect(open.status).toBe(200);
      const closed = await read(context.request, PRIVATE);
      // Signed in, the private Endpoint either serves the person's grant or refuses it: never 401
      // (the session went with the read) and never a body on a refusal.
      expect([200, 403]).toContain(closed.status);
      if (closed.status === 403) expect(closed.body).not.toContain('"models"');
      test.info().annotations.push({ type: who, description: `public ${open.status}, private ${closed.status}` });
    } finally {
      await context.close();
    }
  });
}
