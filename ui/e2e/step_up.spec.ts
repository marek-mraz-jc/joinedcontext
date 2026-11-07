import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5567: an approver whose right comes from a role binding signed in with a password alone.
// The approve door answers 403 second-factor-required; the dialog sends them to the step-up
// login in a new tab, says when the code has not arrived yet, and lets the decision through after.
// The API is stubbed in the browser, as in approvals.spec.ts.

const CHANGE = "chg-1a2b3c4d";
const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: CHANGE, namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Policy", name: "air-quality-write", fields: 1 } },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-10-24T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
  planFields: [{ path: "spec.rules", from: "read", to: "read, write" }],
};
const REFUSAL = {
  type: "https://joinedcontext.com/errors/second-factor-required",
  title: "Second Factor Required",
  status: 403,
  detail: "This decision needs a second factor in this session: sign in again with your one-time code, then repeat it.",
};

async function stubApi(page: Page, state: { factor: boolean }): Promise<string[]> {
  const approvals: string[] = [];
  // The step-up tab opens the realm's login; here it only has to open.
  await page.context().route("**/api/v1/auth/login**", (route) =>
    route.fulfill({ status: 200, contentType: "text/html", body: "<title>Sign in</title>" }),
  );
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200, contentType = "application/json") =>
      route.fulfill({ status, contentType, body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({
        subject: "b7c1e0f4",
        username: "jana.kovacova",
        name: "Jana Kováčová",
        email: "jana.kovacova@banskabystrica.sk",
        roles: ["portal-approver"],
        second_factor: state.factor,
      });
    }
    if (url.pathname.endsWith("/approve")) {
      approvals.push(url.pathname);
      if (!state.factor) return json(REFUSAL, 403, "application/problem+json");
      return json({ ...PROPOSAL, kind: "Change", status: { lane: "yellow", phase: "Deploying", plan: { update: 1 } } }, 202);
    }
    if (url.pathname === "/api/v1/projects") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    }
    if (url.pathname.endsWith(`/changes/${CHANGE}`)) return json(PROPOSAL);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return approvals;
}

for (const width of [1440, 400]) {
  test(`a decision refused for want of a second factor steps up and goes through at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const state = { factor: false };
    const approvals = await stubApi(page, state);
    await page.goto(`/projects/helsinki/approvals/${CHANGE}?lang=en`);

    await page.getByRole("button", { name: "Approve" }).click();
    const dialog = page.getByRole("dialog", { name: "Confirm with your one-time code" });
    await expect(dialog).toBeVisible();
    const signIn = dialog.getByRole("link", { name: "Sign in with the code in a new tab" });
    await expect(signIn).toHaveAttribute(
      "href",
      `/api/v1/auth/login?step_up=true&redirect_to=${encodeURIComponent(`/projects/helsinki/approvals/${CHANGE}`)}%3Flang%3Den`,
    );
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: test.info().outputPath(`step-up-refused-${width}.png`), fullPage: true });

    const popup = page.waitForEvent("popup");
    await signIn.click();
    await (await popup).close();
    await dialog.getByRole("button", { name: "Continue here" }).click();
    await expect(dialog.getByText("The code has not arrived yet.", { exact: false })).toBeVisible();
    await page.screenshot({ path: test.info().outputPath(`step-up-not-yet-${width}.png`), fullPage: true });

    state.factor = true;
    await dialog.getByRole("button", { name: "Continue here" }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByText("Deploying")).toBeVisible();
    expect(approvals).toHaveLength(2);
    await page.screenshot({ path: test.info().outputPath(`step-up-approved-${width}.png`), fullPage: true });
  });
}
