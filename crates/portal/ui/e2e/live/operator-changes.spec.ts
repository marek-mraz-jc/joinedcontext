/**
 * T-7139 — operator Changes on dev (evidence/T-7129/design.md §4).
 *
 * A host of an Organization and a member of the operators' group change only through an operator
 * Change that a second operator approves. `demo.viewer` is a signed-in person of no operators'
 * group: the Portal refuses them the list, the proposal, the approval and the rejection, so they
 * neither read nor change the host registry. The two-operator path needs two operators with a
 * second factor in the run; tests/operator_changes_tests.rs proves it against a test database.
 */
import { expect, test } from "@playwright/test";
import { VIEWER, csrf, signIn } from "./portal";

test("a person of no operators' group is refused every operator Change door", async ({ browser }) => {
  test.skip(!VIEWER.password, "VIEWER_PASSWORD is not set");
  const { context, page } = await signIn(browser, VIEWER, "/?lang=en");
  const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
  const id = "00000000-0000-0000-0000-000000000000";
  const answers = [
    ["GET /api/v1/operator/changes", await page.request.get("/api/v1/operator/changes")],
    [
      "POST /api/v1/operator/changes",
      await page.request.post("/api/v1/operator/changes", {
        headers,
        data: { kind: "HostAdd", organization: "demo", host: "viewer.example.org", role: "working" },
      }),
    ],
    [`POST .../${id}/approve`, await page.request.post(`/api/v1/operator/changes/${id}/approve`, { headers })],
    [`POST .../${id}/reject`, await page.request.post(`/api/v1/operator/changes/${id}/reject`, { headers })],
  ] as const;
  for (const [door, answer] of answers) {
    expect(answer.status(), `${door} refuses a non-operator`).toBe(403);
    expect(await answer.text(), `${door} says who may`).toContain("operators");
  }
  await context.close();
});
