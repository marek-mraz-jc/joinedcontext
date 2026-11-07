import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-5829 (Antares T-5816): the page a context broker sends the person to when their AI client
// asks for an MCP write. The person reads the view, the tool and the words the client was shown,
// and accepts or declines with their session; the Portal records it on the broker. `vite preview`
// has no Portal API behind it, so the API is answered in the browser; the page is the real build.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana@hel.fi", roles: [] };
const ID = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
const QUESTION = {
  id: ID,
  view: "bb-ovzdusie-air-quality-observed",
  tool: "create_entity",
  message: "Create the Entity urn:ngsi-ld:AirQualityObserved:bb-namestie-snp-2026-10-27 in view bb-ovzdusie-air-quality-observed.",
  expiresAt: Math.floor(Date.now() / 1000) + 600,
  decision: null as string | null,
};
const SHOTS = "test-results/mcp-confirmation";

async function stubApi(page: Page, status = 200): Promise<string[]> {
  const decided: string[] = [];
  let question = { ...QUESTION };
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, code = 200) =>
      route.fulfill({ status: code, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === `/api/v1/mcp-confirmations/${ID}`) {
      if (status !== 200) return json({ type: "about:blank", title: "Error", status, detail: "a context broker did not answer, so the question may be on it; try again in a minute" }, status);
      return json(question);
    }
    if (path === `/api/v1/mcp-confirmations/${ID}/decision` && request.method() === "POST") {
      const body = request.postDataJSON() as { decision: string };
      expect(Object.keys(body)).toEqual(["decision"]);
      decided.push(body.decision);
      question = { ...question, decision: body.decision };
      return json(question);
    }
    return json({ kind: "List", items: [] });
  });
  return decided;
}

for (const width of [1440, 400]) {
  test.describe(`the broker MCP write at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("the person reads the write, accepts it, changes their mind and declines it", async ({ page }) => {
      const decided = await stubApi(page);
      await page.goto(`/mcp-confirmations/${ID}?lang=en`);
      await expect(page.getByRole("heading", { level: 1, name: "Decide an AI client's call" })).toBeVisible();
      await expect(page.getByTestId("mcp-elicitation-message")).toContainText("AirQualityObserved:bb-namestie-snp");
      await expect(page.getByText("create_entity")).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/question-${width}.png`, fullPage: true });

      await page.getByRole("button", { name: "Accept the call" }).click();
      await expect(page.getByText("You accepted this call")).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/accepted-${width}.png`, fullPage: true });

      await page.getByRole("button", { name: "Decline the call" }).click();
      await expect(page.getByText("You declined this call")).toBeVisible();
      expect(decided).toEqual(["accept", "decline"]);
    });

    test("a question not open for this person says what to do next", async ({ page }) => {
      await stubApi(page, 404);
      await page.goto(`/mcp-confirmations/${ID}?lang=en`);
      await expect(page.getByRole("heading", { name: "This question is no longer open" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Accept the call" })).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Back to the Portal" })).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/gone-${width}.png`, fullPage: true });
    });

    test("a broker that did not answer offers a retry", async ({ page }) => {
      await stubApi(page, 503);
      await page.goto(`/mcp-confirmations/${ID}?lang=en`);
      await expect(page.getByText("try again in a minute")).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/failed-${width}.png`, fullPage: true });
    });
  });
}
