/**
 * Ready for the demo, step 6 with the model (T-2746, TS-01): an app built from the assistant's
 * "Build an app" path, published, approved and built, opens inside the Portal and in a window of
 * its own, each behind a login, and the viewer opens it without the steward's role.
 *
 * It spends the model (the build and its follow-ups), so it is nightly: the file is in
 * `sweep.sh`'s spending list and its title names the assistant. The refusal of a person outside
 * the app's roles is walked hourly in `readiness.spec.ts` on the seeded `visibility: roles` app:
 * a generated app is `project` or `organization` (AP-42), which every signed-in person opens, and
 * the endpoint's grants decide what they read (AP-18).
 */
import { expect, test } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, goSignedIn, removeCompletely, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const NAME = `rdy-app-${SUFFIX}`;
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";
const PROMPT = "A table of the HSL city bike stations with their free bikes, and a map of them.";

test.setTimeout(1_800_000);

test("the assistant's Build an app path makes an app that opens in the Portal and on its own, behind a login", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  try {
    // The path's first question is the Portal's own (no model call): which endpoints to read.
    await page.getByRole("button", { name: "Open the assistant" }).click();
    await page.getByTestId("assistant-paths").getByRole("button", { name: /^Build an app/ }).click();
    const question = page.getByRole("group", { name: "Which endpoints should the app read?" });
    await question.getByRole("button", { name: /bike/i }).first().click();
    await page.getByRole("button", { name: /^Use these \(1\)$/ }).click();

    // The builder opens with the endpoint chosen; the rest is the builder's own form.
    const builder = page.getByTestId("assistant-build");
    await expect(builder).toBeVisible({ timeout: 60_000 });
    await builder.getByLabel("What should the app do?").fill(PROMPT);
    await builder.getByText("Details:", { exact: false }).first().click();
    await builder.getByLabel("App name").fill(NAME);
    await builder.getByRole("button", { name: "Generate the app" }).click();
    await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${NAME}`), { timeout: 60_000 });
    await expect(page.frameLocator("iframe").first().locator("table, svg, canvas").first()).toBeVisible({ timeout: 600_000 });

    // Published: the merge request opens and another person approves (CC-34).
    await page.getByRole("button", { name: "Publish this app" }).click();
    await page.getByRole("button", { name: "Open the merge request" }).click();
    const notice = page.getByRole("link", { name: /review/i }).first();
    await expect(notice).toBeVisible({ timeout: 120_000 });
    const change = new URL((await notice.getAttribute("href")) ?? "", page.url()).pathname.split("/").pop() ?? "";
    expect(change, "Publish left a change to approve").not.toBe("");
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    await approve(approver.page, PROJECT, change, NAME);
    await approver.context.close();

    // Built: the manifest's status names the commit the lane served.
    await expect
      .poll(
        async () => {
          const answer = await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`);
          const app = answer.ok() ? ((await answer.json()) as { status?: { build?: { commit?: string } } }) : {};
          return app.status?.build?.commit ?? "";
        },
        { timeout: 900_000, intervals: [15_000], message: "the app is built" },
      )
      .toMatch(/^[0-9a-f]{7}/);

    // Inside the Portal, framed, and in a window of its own.
    await page.goto(`/projects/${PROJECT}/apps/${NAME}/open?lang=en`, { waitUntil: "load" });
    await expect(page.locator("iframe[sandbox]")).toHaveAttribute("sandbox", "allow-scripts allow-forms allow-popups allow-downloads");
    await expect(page.frameLocator("iframe[sandbox]").getByRole("heading", { level: 1 })).toBeVisible({ timeout: 120_000 });
    const popup = page.waitForEvent("popup");
    await page.getByRole("link", { name: /Open in new window/ }).click();
    const own = await popup;
    await own.waitForURL((url) => url.pathname === `/apps/${NAME}/`, { timeout: 60_000 });
    await own.close();

    // Nobody signed in meets a login: at the app and at the Portal's page of it.
    const anonymous = await browser.newContext();
    try {
      const stranger = await anonymous.newPage();
      await stranger.goto(`${APPS_URL}/apps/${NAME}/`, { waitUntil: "load" });
      await expect(stranger.locator("#username")).toBeVisible({ timeout: 60_000 });
      await stranger.goto(`/projects/${PROJECT}/apps/${NAME}/open?lang=en`, { waitUntil: "load" });
      await expect(stranger.locator("#username").or(stranger.getByRole("button", { name: "Sign in" })).first()).toBeVisible({
        timeout: 60_000,
      });
    } finally {
      await anonymous.close();
    }

    // The viewer opens it, without the steward's role.
    const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
    try {
      await goSignedIn(viewer.page, VIEWER, `${APPS_URL}/apps/${NAME}/`, (opened) => opened.getByRole("heading", { level: 1 }));
      const config = JSON.parse((await viewer.page.locator("#jc-config").textContent()) ?? "{}") as { user?: { roles?: string[] } };
      expect(config.user?.roles ?? []).not.toContain("steward");
    } finally {
      await viewer.context.close();
    }
  } finally {
    await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
    await removeCompletely(steward, PROJECT, "apps", NAME);
    await sweepDrafts(steward.context, page, PROJECT, new RegExp(`^${NAME}$`));
    await steward.context.close();
  }
});

test("a person stops an app run the assistant started, and the run says it was stopped", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  const stopped = `rdy-stop-${SUFFIX}`;
  try {
    await page.getByRole("button", { name: "Generate your own app" }).first().click();
    const endpoint = page.getByLabel("Endpoint").first();
    const bikes = endpoint.locator("option").filter({ hasText: /bike/i }).first();
    await endpoint.selectOption((await bikes.getAttribute("value")) ?? "");
    await page.getByLabel("What should the app do?").first().fill(PROMPT);
    await page.getByText("Details:", { exact: false }).first().click();
    await page.getByLabel("App name").fill(stopped);
    await page.getByRole("button", { name: "Generate the app" }).click();
    await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${stopped}`), { timeout: 60_000 });
    // Stopped as soon as it may be, so the run spends as little of the model as it can.
    const stop = page.getByRole("button", { name: "Stop the run" });
    await expect(stop).toBeEnabled({ timeout: 60_000 });
    await stop.click();
    await expect(page.getByText("The run was stopped.")).toBeVisible({ timeout: 60_000 });
    await expect(stop).toBeDisabled();
  } finally {
    // Nothing was published, so no App exists; only the run's drafts could be left.
    await sweepDrafts(steward.context, page, PROJECT, new RegExp(`^${stopped}$`));
    await steward.context.close();
  }
});
