/**
 * T-4849: Administration → Agents (AG-126…AG-129, API/01 §48). One table of every agent the
 * person may read; an agent's page with Overview, Access, Activity and Settings; Pause and Resume
 * of an assistant as a Change sent for review, shown only to a person who may propose it.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { AgentPage, Agents } from "../src/pages/organization/Agents";
import { ServiceAccounts } from "../src/pages/access/ServiceAccounts";
import { McpServersPage } from "../src/pages/mcp/McpServersPage";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const t = en.organization.agents;

const STEWARD = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["AssistantDeployment"], verbs: ["read", "propose"] } }],
} as unknown as Effective;

const VIEWER = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "viewer", binding: "viewers", rule: { kinds: ["AssistantDeployment"], verbs: ["read"] } }],
} as unknown as Effective;

const CHAT = {
  kind: "AssistantDeployment",
  name: "city-chat",
  project: "helsinki",
  title: "City chat",
  owner: { type: "project", name: "helsinki" },
  actsAs: "self",
  spaces: ["air"],
  endpoints: ["air-quality"],
  lastActivity: null,
  spend: { used: null, cap: 200000, per: "day" },
  state: "active",
  runId: null,
  profile: null,
};

const ACCOUNT = {
  ...CHAT,
  kind: "ServiceAccount",
  name: "sensor-feed",
  title: null,
  owner: { type: "person", name: "jana@hel.fi" },
  endpoints: [],
  spend: { used: null, cap: null, per: null },
};

const RUN = {
  ...CHAT,
  kind: "AgentRun",
  name: "Air quality dashboard",
  title: null,
  owner: { type: "person", name: "ana@hel.fi" },
  actsAs: "caller",
  lastActivity: "2026-10-21T02:10:00Z",
  spend: { used: 41000, cap: 400000, per: "run" },
  state: "running",
  runId: "run-7f3a",
  profile: "builder",
};

const STORED = {
  apiVersion: "joinedcontext.com/v1",
  kind: "AssistantDeployment",
  metadata: { name: "city-chat", namespace: "helsinki" },
  spec: { publicId: "city-chat", channel: "public", enabled: true, systemPrompt: "SYSTEM-PROMPT-TEXT", budget: { tokensPerDay: 200000 } },
};

interface Harness {
  items?: unknown[];
  status?: number;
  permissions?: Effective;
  page?: "list" | string;
}

function render({ items = [CHAT, ACCOUNT, RUN], status = 200, permissions = STEWARD, page = "list" }: Harness) {
  const puts: { url: URL; body: unknown }[] = [];
  const element = page === "list" ? <Agents /> : <AgentPage path={page} />;
  const result = renderPage(element, {
    path: "/organization/agents",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/agents") {
        return status === 200
          ? json({ items })
          : json(
              { type: "about:blank", title: "Forbidden", status, detail: "your roles read no agent in any project" },
              status,
            );
      }
      if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts/sensor-feed/access") {
        return json({
          account: "sensor-feed",
          project: "helsinki",
          delegates: false,
          mayPropose: true,
          mayRevoke: false,
          grants: [{ index: 0, role: "space-writer", scope: "project:helsinki", state: "active", rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }] }],
        });
      }
      if (url.pathname === "/api/v1/projects/helsinki/assistantdeployments/city-chat") {
        if (request.method === "PUT") {
          puts.push({ url, body: await request.json() });
          return url.searchParams.get("dryRun")
            ? json({ valid: true, verdict: { ok: true } })
            : json({ metadata: { name: "chg-42" }, status: "open" }, 202);
        }
        return json(STORED);
      }
      return undefined;
    },
  });
  return { ...result, puts };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Administration → Agents", () => {
  it("lists each agent with its kind, owner, whom it acts as, reach, spend and state", async () => {
    const { container } = render({});
    const table = await screen.findByRole("table", { name: t.caption });
    await within(table).findByRole("link", { name: "City chat" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(4);
    const chat = rows[1];
    expect(within(chat).getByRole("link", { name: "City chat" })).toBeInTheDocument();
    expect(within(chat).getByText(`${t.kind.AssistantDeployment} · helsinki`)).toBeInTheDocument();
    expect(within(chat).getByText("Project helsinki")).toBeInTheDocument();
    expect(within(chat).getByText(t.actsAs.self)).toBeInTheDocument();
    expect(within(chat).getByText("Spaces: 1 · Endpoints: 1")).toBeInTheDocument();
    expect(within(chat).getByText("Cap 200,000 tokens a day")).toBeInTheDocument();
    expect(within(chat).getByText(t.state.active)).toBeInTheDocument();
    expect(within(rows[2]).getByText("jana@hel.fi")).toBeInTheDocument();
    // The phone list carries the same agents, one card each.
    const cards = within(screen.getByRole("list", { name: t.caption })).getAllByRole("listitem");
    expect(cards).toHaveLength(3);
    expect(within(cards[2]).getByText("Spaces: 1 · Endpoints: 1 · 41,000 of 400,000 tokens")).toBeInTheDocument();
    expect(within(rows[2]).getByText(t.spendNone)).toBeInTheDocument();
    expect(within(rows[3]).getByText("41,000 of 400,000 tokens")).toBeInTheDocument();
    expect(within(rows[3]).getByText(t.actsAs.caller)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("shows a designed empty state when no agent is declared", async () => {
    render({ items: [] });
    expect(await screen.findByText(t.empty)).toBeInTheDocument();
    expect(screen.getByText(t.emptyHint)).toBeInTheDocument();
  });

  it("says who can grant access when the person reads no agent kind", async () => {
    render({ status: 403 });
    expect(await screen.findByText(/your roles read no agent in any project/)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });
});

describe("An agent's page", () => {
  it("has the four tabs and never shows the system prompt", async () => {
    const { container } = render({ page: "helsinki/AssistantDeployment/city-chat" });
    expect(await screen.findByRole("heading", { level: 1, name: "City chat" })).toBeInTheDocument();
    const tabs = screen.getByRole("tablist", { name: t.tabsLabel });
    expect(within(tabs).getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
      t.tab.overview,
      t.tab.access,
      t.tab.activity,
      t.tab.settings,
    ]);
    fireEvent.click(within(tabs).getByRole("tab", { name: t.tab.access }));
    expect(screen.getByText("air-quality")).toBeInTheDocument();
    fireEvent.click(within(tabs).getByRole("tab", { name: t.tab.settings }));
    expect(await screen.findByText(t.setting.budget.tokensPerDay)).toBeInTheDocument();
    expect(screen.getByText("200000")).toBeInTheDocument();
    expect(container.textContent).not.toContain("SYSTEM-PROMPT-TEXT");
    await expectNoAxeViolations(container);
  });

  it("pauses an assistant as a checked Change with enabled false", async () => {
    const { puts } = render({ page: "helsinki/AssistantDeployment/city-chat" });
    fireEvent.click(await screen.findByRole("button", { name: t.pause }));
    expect(await screen.findByText(t.pauseSent)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: t.openChange })).toHaveAttribute(
      "href",
      "/projects/helsinki/approvals/chg-42",
    );
    expect(puts.map((put) => put.url.searchParams.get("dryRun"))).toEqual(["All", null]);
    const sent = puts[1].body as { spec: { enabled: boolean; systemPrompt: string } };
    expect(sent.spec.enabled).toBe(false);
    // The rest of the manifest goes back as stored: a pause changes one field.
    expect(sent.spec.systemPrompt).toBe("SYSTEM-PROMPT-TEXT");
  });

  it("offers Resume for a paused assistant", async () => {
    render({ items: [{ ...CHAT, state: "paused" }], page: "helsinki/AssistantDeployment/city-chat" });
    expect(await screen.findByRole("button", { name: t.resume })).toBeInTheDocument();
  });

  it("shows a viewer the state and no Pause", async () => {
    render({ permissions: VIEWER, page: "helsinki/AssistantDeployment/city-chat" });
    expect(await screen.findByRole("heading", { level: 1, name: "City chat" })).toBeInTheDocument();
    await screen.findByText(t.state.active);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("button", { name: t.pause })).toBeNull();
  });

  it("says a kind without an off switch is removed on its own page", async () => {
    render({ page: "helsinki/ServiceAccount/sensor-feed" });
    expect(await screen.findByText(t.noPause)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: t.pause })).toBeNull();
  });

  it("shows a service account's grants and kind × verb matrix on its Access tab (T-4850)", async () => {
    const { container } = render({ page: "helsinki/ServiceAccount/sensor-feed" });
    const tabs = await screen.findByRole("tablist", { name: t.tabsLabel });
    fireEvent.click(within(tabs).getByRole("tab", { name: t.tab.access }));
    const matrix = await screen.findByRole("group", { name: "What sensor-feed may do, by kind" });
    expect(within(matrix).getByRole("row", { name: /Pipeline/ })).toBeInTheDocument();
    expect(screen.getByText("space-writer")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Revoke/ })).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("tells the person when the agent is no longer there", async () => {
    render({ page: "helsinki/AssistantDeployment/gone" });
    expect(await screen.findByRole("heading", { level: 1, name: t.gone })).toBeInTheDocument();
  });
});

describe("Administration's own lists say they are the organization's (T-4848 finding 4)", () => {
  it("names the organization, not a project, when it has no service account or MCP server", async () => {
    renderPage(
      <>
        <ServiceAccounts project="org" />
        <McpServersPage project="org" embedded />
      </>,
      {
        path: "/organization/service-accounts",
        answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(STEWARD) : undefined),
      },
    );
    expect(await screen.findByText(en.access.accounts.emptyOrganization)).toBeInTheDocument();
    expect(await screen.findByText(en.mcp.emptyOrganization)).toBeInTheDocument();
    expect(screen.queryByText(en.access.accounts.empty)).toBeNull();
  });
});
