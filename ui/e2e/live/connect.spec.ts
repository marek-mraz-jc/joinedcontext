/**
 * T-4169 — the public MCP setup page on dev: a visitor with no session opens the region's
 * connector page, reads its tools from the live server and finds the Claude Code command.
 */
import { expect, test } from "@playwright/test";

test.setTimeout(120_000);

test("a visitor opens the region's public MCP connector page", async ({ browser }) => {
  const visitor = await browser.newContext();
  try {
    const page = await visitor.newPage();
    await page.goto(`/connect/bbsk/otvorene-data?lang=sk`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1, name: "Pripojte AI asistenta" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByText("query_entities", { exact: true })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByText(/^claude mcp add --transport http bbsk-otvorene-data https:\/\//)).toBeVisible();
  } finally {
    await visitor.close();
  }
});

/**
 * T-4762 (owner, prod, 2026-10-19): the catalogue's "Pripojte AI asistenta" card on /connector.
 * Each Kopírovať button puts exactly its text on the clipboard: the address, the Claude Code
 * command on one line, and JSON that parses. CKAN_URL picks the catalogue (prod:
 * https://opendata.smartcity.joinedcontext.com).
 */
const CATALOGUE = (process.env.CKAN_URL ?? "https://opendata.dev.joinedcontext.com").replace(/\/$/, "");

test("the catalogue's connector card copies the address, the command and the JSON", async ({ browser }) => {
  const visitor = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
  try {
    const page = await visitor.newPage();
    await page.goto(`${CATALOGUE}/connector?lang=sk`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 2, name: "Pripojte AI asistenta" })).toBeVisible({ timeout: 60_000 });
    const copied = async (target: string): Promise<string> => {
      await page.evaluate(() => navigator.clipboard.writeText(""));
      const button = page.locator(`[data-jc-copy="${target}"]`);
      await button.click();
      await expect(button).toContainText("Skopírované");
      await expect(page.locator("#jc-copy-status")).toHaveText("Skopírované");
      return page.evaluate(() => navigator.clipboard.readText());
    };
    const url = await copied("jc-mcp-url");
    expect(url).toMatch(/^https:\/\/api\.[a-z0-9.-]+\/api\/mcp(\/d\/[a-z0-9-]+)?$/);
    expect(await copied("jc-mcp-claude")).toBe(`claude mcp add --transport http joinedcontext ${url}`);
    expect(JSON.parse(await copied("jc-mcp-json"))).toEqual({ mcpServers: { joinedcontext: { type: "http", url } } });
  } finally {
    await visitor.close();
  }
});
