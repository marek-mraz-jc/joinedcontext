/**
 * T-5699, AG-63 — the page where a person decides an AI client's MCP call. A question is the
 * person's own: one that is not open for them (answered, expired, or another person's client
 * asked it) shows what to do next, and their session decides nothing on it. The success path
 * needs a person's own MCP token, which no live account holds; tests/mcp_portal_tests.rs plays it
 * against the real route, and ui/e2e/mcp-elicitation.spec.ts on the page.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

test.setTimeout(120_000);

test("a person opens an MCP call's question that is not open for them, is told what to do, and decides nothing", async ({ browser }, info) => {
  const address = "/projects/helsinki/mcp-elicitations/eli-0000000000000000?lang=en";
  const { context, page } = await signIn(browser, STEWARD, address);
  try {
    await expect(page.getByRole("heading", { name: "This question is no longer open" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByText("Ask your AI client to send its call again")).toBeVisible();
    await info.attach("mcp-question-gone.png", { body: await page.screenshot(), contentType: "image/png" });
    const decided = await page.request.post("/api/v1/projects/helsinki/mcp-elicitations/eli-0000000000000000/decision", {
      data: { decision: "accept" },
      headers: { "x-csrf-token": await csrf(context) },
    });
    expect(decided.status()).toBe(404);
  } finally {
    await context.close();
  }
});

// T-5829: the broker's question (Antares T-5816) is read from the broker; an id no broker holds
// for this person is the same "no longer open" page, and the session decides nothing on it.
test("a person opens a broker MCP write's question that is not open for them, is told what to do, and decides nothing", async ({ browser }, info) => {
  const id = "00000000000000000000000000000000";
  const { context, page } = await signIn(browser, STEWARD, `/mcp-confirmations/${id}?lang=en`);
  try {
    await expect(page.getByRole("heading", { name: "This question is no longer open" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("link", { name: "Back to the Portal" })).toBeVisible();
    await info.attach("mcp-confirmation-gone.png", { body: await page.screenshot(), contentType: "image/png" });
    const decided = await page.request.post(`/api/v1/mcp-confirmations/${id}/decision`, {
      data: { decision: "accept" },
      headers: { "x-csrf-token": await csrf(context) },
    });
    expect(decided.status()).toBe(404);
  } finally {
    await context.close();
  }
});
