/**
 * The Assistant page on dev (T-2729; UI-54, AG-71, TS-26): a steward opens it, asks, gets an
 * answer in words, and finds the conversation listed on the page afterwards; a viewer, who may
 * not propose an App, is refused a conversation with the reason.
 *
 * The question is a read — which endpoints publish air quality — so nothing is proposed and the
 * journey may run at any time. It spends one model call: every title here names the assistant,
 * so the hourly sweep leaves it to the nightly eval batch (`sweep.sh`, T-2733).
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, ask, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
/** The first message names the conversation on the page, so it carries the run's own mark. */
const QUESTION = `Which endpoints of ${PROJECT} publish air quality? (journey ${SUFFIX})`;

test.setTimeout(900_000);

test("a steward asks from the assistant page, gets an answer, and finds the conversation listed", async ({ browser }) => {
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/assistant?lang=en`);
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Assistant" })).toBeVisible({ timeout: 60_000 });

    await ask(page, QUESTION);
    // The assistant's turn in the dock's conversation, never the sidebar's "Assistant" link, which
    // matched at once and sent the journey off the page before the question was even posted. A
    // half-written answer is hidden from the accessibility tree, so this waits for the finished one.
    const answer = page
      .getByRole("list", { name: "Conversation" })
      .getByRole("listitem")
      .filter({ hasText: /^Assistant/ })
      .last();
    await expect(answer).toBeVisible({ timeout: 300_000 });
    await expect(answer).not.toContainText(/cannot|not allowed|forbidden|does not grant|The answer failed/i);
    await expect(page.getByRole("alert")).toHaveCount(0);

    await page.goto(`/projects/${PROJECT}/assistant?lang=en`, { waitUntil: "load" });
    // A journey's run is a test run, which the page lists only on request (AG-93).
    await page.getByRole("checkbox", { name: "Show test runs" }).check();
    const table = page.getByRole("table", { name: "Assistant" });
    await expect(table.getByRole("row").filter({ hasText: `journey ${SUFFIX}` })).toHaveCount(1, { timeout: 60_000 });
  } finally {
    await context.close();
  }
});

test("a viewer is refused an assistant conversation with the reason", async ({ browser }) => {
  const { page, context } = await signIn(browser, VIEWER, `/projects/${PROJECT}/assistant?lang=en`);
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Assistant" })).toBeVisible({ timeout: 60_000 });
    const answer = await page.request.post(`/api/v1/projects/${PROJECT}/assistant/conversations`, {
      headers: { "x-csrf-token": await csrf(context) },
      data: { message: QUESTION },
    });
    expect(answer.status(), await answer.text()).toBe(403);
    // The reason names what the role lacks, so the person knows whom to ask (UI-44).
    expect(await answer.text()).toMatch(/propose/i);
  } finally {
    await context.close();
  }
});
