import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-5699, AG-63: the page behind an AI client's MCP question. The person reads what the call
// would do, opens what it changes, and accepts or declines with their session; the decision is
// what the client's next call reads. `vite preview` has no Portal API behind it, so the API is
// answered in the browser; the page is the real build.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-approver"] };
const ID = "eli-0a1b2c3d4e5f6a7b";
const QUESTION = {
  id: ID,
  operation: "jc_resource_delete",
  message: "Delete Resource in project 'helsinki' (red lane). Nothing has run: open the Portal page this question names, look at what this would change and decide there, then send this call again with the `elicitationId`.",
  resource: "/projects/helsinki/endpoints/helsinki-bikes",
  expiresAt: Math.floor(Date.now() / 1000) + 600,
  decision: null as string | null,
};
const SHOTS = "test-results/mcp-elicitation";

async function stubApi(page: Page, gone = false): Promise<string[]> {
  const decided: string[] = [];
  let question = { ...QUESTION };
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === `/api/v1/projects/helsinki/mcp-elicitations/${ID}`) {
      if (gone) return json({ type: "about:blank", title: "Not Found", status: 404, detail: "no open question" }, 404);
      return json(question);
    }
    if (path === `/api/v1/projects/helsinki/mcp-elicitations/${ID}/decision` && request.method() === "POST") {
      const { decision } = request.postDataJSON() as { decision: string };
      decided.push(decision);
      question = { ...question, decision };
      return json(question);
    }
    return json({ kind: "List", items: [] });
  });
  return decided;
}

for (const width of [1440, 400]) {
  test.describe(`the MCP question at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("the person reads the call, accepts it, changes their mind and declines it", async ({ page }) => {
      const decided = await stubApi(page);
      await page.goto(`/projects/helsinki/mcp-elicitations/${ID}?lang=en`);
      await expect(page.getByRole("heading", { level: 1, name: "Decide an AI client's call" })).toBeVisible();
      await expect(page.getByTestId("mcp-elicitation-message")).toContainText("Delete Resource in project 'helsinki'");
      await expect(page.getByRole("link", { name: "Look at what this call changes" })).toHaveAttribute(
        "href",
        "/projects/helsinki/endpoints/helsinki-bikes",
      );
      await page.screenshot({ path: `${SHOTS}/question-${width}.png`, fullPage: true });

      await page.getByRole("button", { name: "Accept the call" }).click();
      await expect(page.getByText("You accepted this call")).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/accepted-${width}.png`, fullPage: true });

      await page.getByRole("button", { name: "Decline the call" }).click();
      await expect(page.getByText("You declined this call")).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/declined-${width}.png`, fullPage: true });
      expect(decided).toEqual(["accept", "decline"]);
    });

    test("a question that is no longer open says what to do next", async ({ page }) => {
      await stubApi(page, true);
      await page.goto(`/projects/helsinki/mcp-elicitations/${ID}?lang=en`);
      await expect(page.getByRole("heading", { name: "This question is no longer open" })).toBeVisible();
      await expect(page.getByText("Ask your AI client to send its call again")).toBeVisible();
      await expect(page.getByRole("button", { name: "Accept the call" })).toHaveCount(0);
      await page.screenshot({ path: `${SHOTS}/gone-${width}.png`, fullPage: true });
    });
  });
}
