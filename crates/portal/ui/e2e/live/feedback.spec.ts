/**
 * T-3272 — a person sends feedback from the page they are on, and an administrator can read the
 * list it lands in. The send itself is answered in this browser (`page.route`): every run of this
 * journey would otherwise file one more proposed task on the board from dev. The first real send
 * is the deployer's, once, recorded in the task.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(120_000);

test("a person sends feedback from a page, without their name, and an administrator reads the list", async ({ browser }, info) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/approvals?lang=en");
  try {
    let sent: Record<string, unknown> | null = null;
    await page.route("**/api/v1/feedback", async (route) => {
      sent = route.request().postDataJSON() as Record<string, unknown>;
      await route.fulfill({ status: 202, contentType: "application/json", body: JSON.stringify({ id: 1 }) });
    });
    await page.getByRole("button", { name: "Send feedback" }).click({ timeout: 60_000 });
    const dialog = page.getByRole("dialog", { name: "Send feedback" });
    await dialog.getByLabel("What happened?").fill("The Approve button stays grey after I type the name");
    await info.attach("feedback-dialog.png", { body: await page.screenshot(), contentType: "image/png" });
    await dialog.getByRole("button", { name: "Send", exact: true }).click();
    await expect(page.getByRole("status").filter({ hasText: "your feedback was sent" })).toBeAttached({ timeout: 30_000 });
    expect(sent).toEqual({ text: "The Approve button stays grey after I type the name", page: "/projects/helsinki/approvals" });

    // The administrators' list and a screenshot read: reads, nothing written.
    const list = await page.request.get("/api/v1/organization/feedback?after=0");
    expect(list.status()).toBe(200);
    expect(Array.isArray(((await list.json()) as { items?: unknown[] }).items)).toBe(true);
    expect((await page.request.get("/api/v1/organization/feedback/0/screenshot")).status()).toBe(404);
  } finally {
    await context.close();
  }
});
