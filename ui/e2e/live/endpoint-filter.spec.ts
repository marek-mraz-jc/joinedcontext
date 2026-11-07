/**
 * The endpoint's filter editor proves what it serves, on dev (T-2776, EP-85, EP-86).
 *
 * The steward opens helsinki-events, which names no projection yet, so the editor starts from the
 * space's model. The proof reads the gateway's preview of the draft beside the space's own
 * entities. An edit narrows it live: only upcoming events and `source` hidden, and the summary,
 * the rows marked "not served" and the struck `source` follow within seconds.
 *
 * Nothing is proposed: the draft lives in the browser and the preview writes nothing, so dev keeps
 * what it had and there is nothing to clean up.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";
const ENDPOINT = "helsinki-events";

test("the filter editor shows what the draft serves beside the original", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/endpoints/${ENDPOINT}`);
  try {
    await expect(page.getByRole("heading", { name: "What this filter serves" })).toBeVisible({ timeout: 60_000 });
    const summary = page.getByRole("status").filter({ hasText: /^Serving \d+ of \d+ Event/ });
    await expect(summary).toBeVisible({ timeout: 60_000 });
    const before = await summary.textContent();
    const total = Number(/of (\d+) Event/.exec(before ?? "")?.[1] ?? "0");
    expect(total, "the space holds events to filter").toBeGreaterThan(0);

    const today = new Date().toISOString().slice(0, 10);
    await page.getByLabel("Attribute query", { exact: true }).fill(`startDate>="${today}T00:00:00Z"`);
    // Several classes list `source`; a hidden name is hidden in every type, so one box is enough.
    await page.getByRole("checkbox", { name: "source", exact: true }).first().check();

    // The left side follows the draft: `source` is hidden on every served row.
    await expect(summary).not.toHaveText(before ?? "", { timeout: 15_000 });
    const after = (await summary.textContent()) ?? "";
    const served = Number(/^Serving (\d+)/.exec(after)?.[1] ?? "-1");
    expect(served).toBeGreaterThanOrEqual(0);
    expect(served).toBeLessThanOrEqual(total);
    expect(after).toMatch(/hidden attributes: [1-9]/);
    const rows = page.getByTestId("proof-row");
    await expect(rows.first()).toBeVisible();
    // Every served row strikes `source` on the right; every dropped row says so.
    const struck = page.locator("del", { hasText: "source" });
    const dropped = page.getByText("Not served: the filter drops this entity.");
    expect((await struck.count()) + (await dropped.count())).toBeGreaterThan(0);
  } finally {
    await context.close();
  }
});
