import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// The public setup page of a named MCP server (T-4169): a visitor with no session reads what the
// connector offers and copies the Claude Code command; the page asks the server without a cookie.
test("a visitor reads a public MCP connector's tools and copies its setup", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.route("**/api/v1/**", async (route) => {
    const unauthorized = new URL(route.request().url()).pathname.endsWith("/auth/me");
    await route.fulfill({
      status: unauthorized ? 401 : 404,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: unauthorized ? "Unauthorized" : "Not Found", status: unauthorized ? 401 : 404 }),
    });
  });
  const cookies: (string | undefined)[] = [];
  await page.route("**/api/mcp/bbsk/otvorene-data", async (route) => {
    cookies.push(route.request().headers()["cookie"]);
    const { method } = route.request().postDataJSON() as { method: string };
    const result =
      method === "initialize"
        ? {
            serverInfo: { name: "otvorene-data", title: "Banská Bystrica Region, open data" },
            instructions: "Read-only open data of the region.\n\nOne server over several Endpoints.",
          }
        : { tools: [{ name: "query_entities", description: "Entities of one type.", annotations: { readOnlyHint: true } }] };
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ jsonrpc: "2.0", id: 1, result }) });
  });

  await page.goto("/connect/bbsk/otvorene-data?lang=en");
  await expect(page.getByRole("heading", { level: 1, name: "Connect an AI assistant" })).toBeVisible();
  await expect(page.getByText("Banská Bystrica Region, open data", { exact: true })).toBeVisible();
  await expect(page.getByText("query_entities", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Copy the command" }).click();
  await expect(page.getByText("Copied to the clipboard.")).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toMatch(
    /^claude mcp add --transport http bbsk-otvorene-data https?:\/\/.+\/api\/mcp\/bbsk\/otvorene-data$/,
  );
  expect(cookies.every((cookie) => cookie === undefined)).toBe(true);
  expect(await axeViolations(page)).toEqual([]);
});
