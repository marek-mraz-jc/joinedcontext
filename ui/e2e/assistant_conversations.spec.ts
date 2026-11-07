/**
 * Asistent → Konverzácie in a browser (T-4681; AG-99, API/01 §34): the administrator opens a
 * stored turn, lists the turns of the same question and compares two side by side, at 1440 and
 * 390 px. The API is answered in the browser; the second run's stored text is hostile and must
 * stay text.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const PROJECT = "helsinki";
const ONE = "11111111-1111-4111-8111-111111111111";
const TWO = "22222222-2222-4222-8222-222222222222";

const step = (kind: string, fields: Record<string, unknown> = {}) => ({
  kind, name: kind, endpoint: null, input: null, output: null, outputBytes: null, status: "ok", startMs: 0, durationMs: 100,
  error: null, stage: null, model: null, provider: null, tokensIn: null, tokensOut: null, reasoningTokens: null,
  costUsd: null, cachedTokens: null, reasoning: null, finishReason: null, effort: null, promptVersion: null, rows: null,
  ...fields,
});
const turn = (answer: string, steps: unknown[], fields: Record<string, unknown> = {}) => ({
  turn: 1, at: "2026-10-18T10:01:00Z", question: "Aké je PM10?", answer, route: "data", firstTokenMs: 800,
  totalMs: 2400, error: null, timedOut: false, costUsd: 0.0046, uncosted: 0, verdict: null, steps, ...fields,
});
const conversation = (id: string, turns: unknown[]) => ({ id, deployment: "obcan", origin: null, turns });

const FIRST = conversation(ONE, [
  turn("PM10 je dnes 31 µg/m³.", [
    step("model", { model: "google/gemini-3.8-flash", tokensIn: 1000, tokensOut: 100, costUsd: 0.0046 }),
    step("tool", { name: "query_entities", endpoint: "hronov-air", input: "{\"type\":\"AirQualityObserved\"}", rows: 1 }),
    step("visual", { name: "chart", input: "{\"mark\":\"line\"}", rows: 12 }),
  ]),
]);
const SECOND = conversation(TWO, [
  turn("PM10 je dnes 40 µg/m³. <img src=x onerror=\"window.pwned=1\">", [
    step("model", { model: "google/gemini-3.8-flash", tokensIn: 1000, tokensOut: 90, cachedTokens: 800, costUsd: 0.006,
      reasoning: "<script>window.pwned=1</script>" }),
    step("visual", { name: "chart", input: "{\"mark\":\"bar\"}", rows: 7 }),
  ], { at: "2026-10-19T10:01:00Z", totalMs: 3100, costUsd: 0.006 }),
]);

async function stubApi(page: Page): Promise<void> {
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "jana", name: "Jana Kováčová", roles: ["portal-approver"] });
      if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
      if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: PROJECT }] });
      if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      if (path.endsWith(`/conversations/${ONE}/turns/1/same`)) {
        return json({ items: [
          { conversation: TWO, turn: 1, at: "2026-10-19T10:01:00Z", deployment: "obcan", totalMs: 3100, costUsd: 0.006, steps: 2 },
          { conversation: ONE, turn: 1, at: "2026-10-18T10:01:00Z", deployment: "obcan", totalMs: 2400, costUsd: 0.0046, steps: 3 },
        ] });
      }
      if (path.endsWith(`/conversations/${ONE}`)) return json(FIRST);
      if (path.endsWith(`/conversations/${TWO}`)) return json(SECOND);
      if (path.endsWith("/knowledge/conversations")) {
        return json({ items: [{
          id: ONE, deployment: "obcan", origin: null, startedAt: "2026-10-18T10:00:00Z", lastAt: "2026-10-18T10:01:00Z",
          firstQuestion: "Aké je PM10?", turns: 1, routes: ["data"], totalMs: 2400, errors: 0, timeouts: 0, costUsd: 0.0046,
          uncosted: 0, thumbsUp: 0, thumbsDown: 0,
        }], nextCursor: null });
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    },
  );
}

for (const width of [1440, 390]) {
  test(`two runs of one question open side by side at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/knowledge/conversations?lang=en`);

    await page.getByRole("table").getByRole("link", { name: "Aké je PM10?" }).click();
    await page.getByRole("button", { name: "Same question" }).click();
    await page.getByRole("list", { name: "Same question" }).getByRole("button", { name: "Compare" }).click();

    const view = page.getByRole("region", { name: "Two answers compared" });
    await expect(view.getByTestId("compare-right-totals")).toContainText("cached 80 %");
    await expect(view.getByRole("list", { name: "Changes in charts, tables and maps" })).toContainText("mark: line → bar");
    await expect(view.getByTestId("answer-diff").locator("ins").first()).toHaveText("40");
    await expect(view).toContainText("<img src=x onerror=");
    expect(await view.locator("img, script").count()).toBe(0);
    expect(await page.evaluate(() => (window as unknown as { pwned?: number }).pwned)).toBeUndefined();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(await axeViolations(page)).toEqual([]);
  });
}
