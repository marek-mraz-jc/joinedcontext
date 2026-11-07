import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-4849, AG-126…AG-129: Administration → Agents in a real browser at 1440 and 400, the API
// answered in the browser as the other mocked specs do: the filled list, an agent's page and its
// pause sent as a Change, the empty list and the refusal. AGENTS_SHOTS names a directory for the
// screenshots the task attaches.

const IDENTITY = { subject: "b7c1e0f4", username: "ida.admin", name: "Ida Admin", email: "ida@hel.fi", roles: [] };
const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: {
        kinds: ["RoleBinding", "AssistantDeployment", "AgentProfile", "McpServer", "ServiceAccount"],
        verbs: ["read", "propose", "approve", "delete"],
      },
    },
  ],
};

const base = {
  project: "helsinki",
  owner: { type: "project", name: "helsinki" },
  actsAs: "caller",
  spaces: [] as string[],
  endpoints: [] as string[],
  lastActivity: null as string | null,
  spend: { used: null as number | null, cap: null as number | null, per: null as string | null },
  state: "active",
  runId: null as string | null,
  profile: null as string | null,
  title: null as string | null,
};

const AGENTS = [
  { ...base, kind: "AgentProfile", name: "app-builder", project: "org", owner: { type: "project", name: "org" },
    lastActivity: "2026-10-21T02:10:00Z", spend: { used: 182400, cap: null, per: "day" } },
  { ...base, kind: "McpServer", name: "helsinki-open-data", project: "org", owner: { type: "project", name: "org" },
    endpoints: ["air-quality", "bike-stations"], spaces: ["air", "mobility"] },
  { ...base, kind: "AssistantDeployment", name: "city-chat", title: "Helsinki city chat", actsAs: "self",
    endpoints: ["air-quality"], spaces: ["air"], lastActivity: "2026-10-21T01:58:00Z",
    spend: { used: null, cap: 200000, per: "day" } },
  { ...base, kind: "AssistantDeployment", name: "staff-helper", title: "Staff helper", state: "paused",
    spend: { used: null, cap: 50000, per: "day" } },
  { ...base, kind: "ServiceAccount", name: "agent-proxy", actsAs: "self", owner: { type: "person", name: "jana.kovacova@hel.fi" },
    spaces: ["air"] },
  { ...base, kind: "AgentRun", name: "Air quality dashboard", owner: { type: "person", name: "jana.kovacova@hel.fi" },
    endpoints: ["air-quality"], spaces: ["air"], lastActivity: "2026-10-21T02:12:00Z",
    spend: { used: 41000, cap: 400000, per: "run" }, state: "running", runId: "run-7f3a", profile: "app-builder" },
];

const DEPLOYMENT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "AssistantDeployment",
  metadata: { name: "city-chat", namespace: "helsinki" },
  spec: { publicId: "city-chat", channel: "public", enabled: true, systemPrompt: "never shown",
    model: { answer: "anthropic/claude-sonnet-5-5", router: "anthropic/claude-haiku-5-5" },
    budget: { tokensPerDay: 200000, costPerDayUsd: 5 } },
};

async function stub(page: Page, agents: { status: number; items: unknown[] }): Promise<string[]> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json(ADMIN);
    if (path === "/api/v1/projects") return json(LIST([{ name: "helsinki" }]));
    if (path === "/api/v1/organization/agents") {
      return agents.status === 200
        ? json({ items: agents.items })
        : json({ type: "about:blank", title: "Forbidden", status: 403,
            detail: "Your roles read no agent in any project: assistant deployments, agent profiles, MCP servers, service accounts and agent runs need read on their kind. Ask a steward of the project or an organization administrator to grant it." }, 403);
    }
    if (path === "/api/v1/projects/helsinki/assistantdeployments/city-chat") {
      if (request.method() === "PUT") {
        writes.push(`PUT ${url.searchParams.get("dryRun") ?? "propose"}`);
        return url.searchParams.get("dryRun")
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change",
              metadata: { name: "chg-000002a1", namespace: "helsinki" }, status: { lane: "green", phase: "PendingApproval" } }, 202);
      }
      return json(DEPLOYMENT);
    }
    return json(LIST([]));
  });
  return writes;
}

const shots = process.env.AGENTS_SHOTS;
async function shoot(page: Page, name: string, width: number): Promise<void> {
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await page.screenshot({ path: join(shots, `agents-${name}-${width}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`Administration → Agents at ${width}`, () => {
    test.use({ viewport: { width, height: width === 400 ? 860 : 900 } });

    test("lists every agent and pauses an assistant as a Change", async ({ page }) => {
      const writes = await stub(page, { status: 200, items: AGENTS });
      await page.goto("/organization/agents");
      // A table at desktop width, one card per agent on a phone.
      const table =
        width === 400
          ? page.getByRole("list", { name: "Agents of the organization" })
          : page.getByRole("table", { name: "Agents of the organization" });
      await expect(table.getByRole("link", { name: "Helsinki city chat" })).toBeVisible();
      await expect(table.getByText("Cap 200,000 tokens a day")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
      expect(overflow).toBe(false);
      await shoot(page, "list", width);

      await table.getByRole("link", { name: "Helsinki city chat" }).click();
      await expect(page.getByRole("heading", { level: 1, name: "Helsinki city chat" })).toBeVisible();
      await expect(page.getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "detail", width);

      await page.getByRole("tab", { name: "Settings" }).click();
      await expect(page.getByText("anthropic/claude-sonnet-5-5")).toBeVisible();
      await expect(page.getByText("never shown")).toHaveCount(0);
      await shoot(page, "settings", width);

      await page.getByRole("tab", { name: "Overview" }).click();
      await page.getByRole("button", { name: "Pause assistant" }).click();
      await expect(page.getByText("Pause sent for review.", { exact: false })).toBeVisible();
      expect(writes).toEqual(["PUT All", "PUT propose"]);
      await shoot(page, "paused-sent", width);
    });

    test("shows the empty state", async ({ page }) => {
      await stub(page, { status: 200, items: [] });
      await page.goto("/organization/agents");
      await expect(page.getByText("No agent yet")).toBeVisible();
      await shoot(page, "empty", width);
    });

    test("tells a person without read on any agent kind who can grant it", async ({ page }) => {
      await stub(page, { status: 403, items: [] });
      await page.goto("/organization/agents");
      await expect(page.getByText("Your roles read no agent in any project", { exact: false })).toBeVisible();
      await shoot(page, "refused", width);
    });
  });
}
