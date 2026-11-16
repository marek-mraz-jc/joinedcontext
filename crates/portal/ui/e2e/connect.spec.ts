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
  // A stranger's Portal knows no open data host: the page says where the address goes, no clicks.
  await expect(page.getByText(/Paste the address into your assistant/)).toBeVisible();
  await expect(page.getByRole("list", { name: "Setup steps per assistant" })).toHaveCount(0);
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: test.info().outputPath(`connect-stranger-copied-${width}.png`), fullPage: true });
  }
  expect(await axeViolations(page)).toEqual([]);
});

// T-7043: a signed-in visitor gets the open data /connector page's section per assistant.
test("a signed-in visitor follows a link to each assistant's steps", async ({ page }) => {
  await page.route("**/api/v1/**", async (route) => {
    if (new URL(route.request().url()).pathname === "/api/v1/organization/addresses") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [{ organization: "bbsk", title: "BBSK", addresses: [{ kind: "opendata", url: "https://data.bbsk.example/" }] }] }),
      });
      return;
    }
    await route.fulfill({ status: 404, contentType: "application/problem+json", body: JSON.stringify({ title: "Not Found", status: 404 }) });
  });
  await page.route("**/api/mcp/bbsk/otvorene-data", async (route) => {
    const { method } = route.request().postDataJSON() as { method: string };
    const result = method === "initialize" ? { serverInfo: { name: "otvorene-data", title: "Open data" }, instructions: "" } : { tools: [] };
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ jsonrpc: "2.0", id: 1, result }) });
  });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/connect/bbsk/otvorene-data?lang=en");
    const guides = page.getByRole("list", { name: "Setup steps per assistant" });
    for (const [name, id] of [["Claude", "claude"], ["ChatGPT", "chatgpt"], ["Grok", "grok"], ["Any other MCP client", "other"]]) {
      await expect(guides.getByRole("link", { name: new RegExp(`^${name}`) })).toHaveAttribute("href", `https://data.bbsk.example/connector#${id}`);
    }
    await page.screenshot({ path: test.info().outputPath(`connect-guides-${width}.png`), fullPage: true });
  }
  expect(await axeViolations(page)).toEqual([]);
});
