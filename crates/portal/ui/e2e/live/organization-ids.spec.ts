/**
 * T-7012 — the Organization id mint on dev (EP-186, PF-41, PF-78; API/15 §2).
 *
 * Minting an id (`bbsk.sk` -> `sk_bbsk`) is the installation operator's decision, taken once per
 * domain after its `_joinedcontext.{domain}` TXT record carries the challenge. `demo.viewer` is a
 * signed-in person of no operators' group: the Portal refuses them the challenge and the mint, so
 * nothing about the domain is stored for them. The success path needs a domain whose DNS the run
 * controls; tests/organization_ids_tests.rs proves it against a stubbed resolver.
 */
import { expect, test } from "@playwright/test";
import { VIEWER, csrf, signIn } from "./portal";

test("a person of no operators' group is refused the Organization id challenge and the mint", async ({ browser }) => {
  test.skip(!VIEWER.password, "VIEWER_PASSWORD is not set");
  const { context, page } = await signIn(browser, VIEWER, "/?lang=en");
  const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
  for (const path of ["/api/v1/organization-ids/challenge", "/api/v1/organization-ids"]) {
    const answer = await page.request.post(path, { headers, data: { domain: "example.org" } });
    expect(answer.status(), `${path} refuses a non-operator`).toBe(403);
    expect(await answer.text(), `${path} says who may mint`).toContain("operators");
  }
  await context.close();
});
