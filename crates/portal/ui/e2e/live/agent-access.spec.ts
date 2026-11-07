/**
 * T-4850, AG-130…AG-133: an agent account's Access view on dev. demo.steward opens the pipeline
 * proposer's grants and matrix, where approve is never held; the account's own token is then
 * refused an approval, a revoke and a new client secret, so an agent neither decides a Change nor
 * changes access.
 * Nothing is changed: the refusals come before any forge call.
 */
import { expect, test } from "@playwright/test";
import { PROPOSER, STEWARD, serviceAccountToken, signIn } from "./portal";

const PROJECT = "helsinki";
const ACCOUNT = "pipeline-proposer";

test("an agent account's token is refused an approval and a revoke, and the steward reads its access", async ({
  browser,
  request,
  baseURL,
}) => {
  test.skip(!STEWARD.password || !PROPOSER.secret, "needs PORTAL_PASSWORD and PROPOSER_CLIENT_SECRET");
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/settings/service-accounts?lang=en`);
  const card = page.getByRole("listitem").filter({ hasText: ACCOUNT });
  const answering = page.waitForResponse((r) => r.url().endsWith(`/serviceaccounts/${ACCOUNT}/access`));
  await card.getByText("What this account may do").click();
  const answer = await answering;
  expect(answer.status()).toBe(200);
  await expect(card.getByText("An account never approves; a person does").first()).toBeVisible();

  const token = await serviceAccountToken(baseURL ?? "");
  const bearer = { Authorization: `Bearer ${token}`, "content-type": "application/json" };
  const approved = await request.post(`/api/v1/projects/${PROJECT}/changes/chg-00000001/approve`, { headers: bearer, data: {} });
  expect(approved.status(), await approved.text()).toBe(403);
  const revoked = await request.post(`/api/v1/projects/${PROJECT}/serviceaccounts/${ACCOUNT}/revoke`, {
    headers: bearer,
    data: { index: 0, role: "pipeline-proposer" },
  });
  expect(revoked.status(), await revoked.text()).toBe(403);
  // Nor does its token ever issue a client secret, its own or another's (T-5626).
  const issued = await request.post(`/api/v1/projects/${PROJECT}/serviceaccounts/${ACCOUNT}/secret`, { headers: bearer });
  expect(issued.status(), await issued.text()).toBe(403);
});
