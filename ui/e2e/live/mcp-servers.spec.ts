/**
 * T-3156 — the named MCP servers on dev (ADR-N-043): the page lists what `/mcpservers` answers the
 * same person, each with its address on the platform host and its sign-in client, and asks every
 * member's own MCP surface as that person. Read only: nothing is proposed here.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";

interface Servers {
  items?: { metadata: { name: string; title?: unknown } }[];
}

test("the MCP servers page agrees with what the API answers, and each server names its address and client", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/mcp?lang=en");
  try {
    await expect(page.getByRole("heading", { level: 1, name: "MCP servers" })).toBeVisible({ timeout: 60_000 });
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/mcpservers`);
    if (!answer.ok()) {
      await expect(page.getByRole("alert")).toContainText("The MCP servers could not be read");
      return;
    }
    const servers = ((await answer.json()) as Servers).items ?? [];
    if (servers.length === 0) {
      await expect(page.getByText("This project has no MCP server yet")).toBeVisible();
      return;
    }
    for (const server of servers) {
      const name = server.metadata.name;
      await expect(page.getByText(`mcp-${PROJECT}-${name}`, { exact: true })).toBeVisible();
      await expect(page.getByText(new RegExp(`/api/mcp/${PROJECT}/${name}$`)).first()).toBeVisible();
    }
    // Every member's health is said in words once its own surface answered this person.
    await expect(page.getByText("Checking…")).toHaveCount(0, { timeout: 30_000 });
  } finally {
    await context.close();
  }
});

