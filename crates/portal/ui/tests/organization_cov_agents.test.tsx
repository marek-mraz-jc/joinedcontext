/**
 * Administration → Agents, the edges of an agent's page: each kind's own Activity and Settings,
 * where an agent is managed, a service account's credentials, and Retry after a failed read.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { AgentPage, Agents, agentPath } from "../src/pages/organization/Agents";
import { json, problem, renderPage } from "./page_contract";

const t = en.organization.agents;

const STEWARD = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["AssistantDeployment"], verbs: ["read", "propose"] } }],
} as unknown as Effective;

const base = {
  kind: "AssistantDeployment",
  name: "city-chat",
  project: "helsinki",
  title: null,
  owner: { type: "project", name: "helsinki" },
  actsAs: "self",
  spaces: [],
  endpoints: [],
  lastActivity: "2026-10-21T02:10:00Z",
  spend: { used: null, cap: null, per: null },
  state: "active",
  runId: null,
  profile: null,
};

const PROFILE = { ...base, kind: "AgentProfile", name: "builder", project: "org", owner: { type: "project", name: "org" } };
const MCP = { ...base, kind: "McpServer", name: "geo-tools", project: "org", owner: { type: "project", name: "org" } };
const ACCOUNT = { ...base, kind: "ServiceAccount", name: "sensor-feed" };
const RUN = { ...base, kind: "AgentRun", name: "Dashboard", actsAs: "caller", runId: "run-7f3a", profile: "builder", state: "running" };
const ITEMS = [PROFILE, MCP, ACCOUNT, RUN];

beforeEach(async () => {
  await i18n.changeLanguage("en");
  try {
    window.localStorage.clear();
  } catch {
    // no storage in this run
  }
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

interface Harness {
  agents?: () => Response;
  manifest?: (url: URL) => Response;
  items?: unknown[];
}

function renderAgent(element: ReactElement, { agents, manifest, items = ITEMS }: Harness = {}) {
  const asked: string[] = [];
  renderPage(element, {
    path: "/organization/agents",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(STEWARD);
      if (url.pathname === "/api/v1/organization/agents") {
        asked.push(url.pathname);
        return agents ? agents() : json({ items });
      }
      if (url.pathname === "/api/v1/organization/agents/activity") return json({ runs: [], alerts: [], choices: { people: [], spaces: [], alertProjects: [] } });
      if (/\/api\/v1\/projects\/[^/]+\/(agentprofiles|serviceaccounts|mcpservers)\//.test(url.pathname) && manifest) {
        asked.push(url.pathname);
        return manifest(url);
      }
      return undefined;
    },
  });
  return asked;
}

describe("Agents: spend in words", () => {
  it("says what was used when no cap is set, a day when the agent names no period", async () => {
    renderAgent(<Agents />, { items: [{ ...base, spend: { used: 1500, cap: null, per: null } }] });
    expect(await screen.findByText("1,500 tokens today")).toBeInTheDocument();
  });

  it("says the run's tokens so far", async () => {
    renderAgent(<Agents />, { items: [{ ...RUN, spend: { used: 900, cap: null, per: "run" } }] });
    expect(await screen.findByText("900 tokens so far")).toBeInTheDocument();
  });
});

describe("An agent's page: where each kind is managed", () => {
  it("points an agent profile's Overview to the profiles page, with no pause", async () => {
    renderAgent(<AgentPage path={agentPath(PROFILE)} />);
    expect(await screen.findByText(t.noPause, { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: t.openHome })).toHaveAttribute("href", "/organization/agentprofiles");
    expect(screen.queryByRole("button", { name: t.pause })).toBeNull();
  });

  it("points an organization MCP server to the organization's list and a project's to its own", async () => {
    renderAgent(<AgentPage path={agentPath(MCP)} />);
    expect(await screen.findByRole("link", { name: t.openHome })).toHaveAttribute("href", "/organization/mcp-servers");
    cleanup();
    renderAgent(<AgentPage path={agentPath({ ...MCP, project: "helsinki" })} />, { items: [{ ...MCP, project: "helsinki" }] });
    expect(await screen.findByRole("link", { name: t.openHome })).toHaveAttribute("href", "/projects/helsinki/mcp");
  });

  it("points a run to its application and says the run stops there", async () => {
    renderAgent(<AgentPage path={agentPath(RUN)} />);
    expect(await screen.findByText(t.runStop, { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: t.openHome })).toHaveAttribute("href", "/projects/helsinki/assistant");
  });
});

describe("An agent's page: Activity and Settings", () => {
  const open = async (tab: string) => {
    fireEvent.click(await screen.findByRole("tab", { name: tab }));
  };

  it("shows an agent profile's runs on its Activity tab", async () => {
    renderAgent(<AgentPage path={agentPath(PROFILE)} />);
    await open(t.tab.activity);
    expect(await screen.findByText(en.organization.agents.activity.none)).toBeInTheDocument();
  });

  it("shows one run's own Activity tab with no filters", async () => {
    renderAgent(<AgentPage path={agentPath(RUN)} />);
    await open(t.tab.activity);
    expect(await screen.findByText(en.organization.agents.activity.none)).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: en.organization.agents.activity.filters })).toBeNull();
  });

  it("sends the other kinds to their own page for what they read and wrote", async () => {
    renderAgent(<AgentPage path={agentPath({ ...MCP })} />, { items: [{ ...MCP, lastActivity: null }] });
    await open(t.tab.activity);
    expect(await screen.findByText(t.activityLead, { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: t.openHome })).toHaveAttribute("href", "/organization/mcp-servers");
    expect(screen.getByText(t.noActivity, { exact: false })).toBeInTheDocument();
  });

  it("says a run follows its agent profile and has no settings of its own", async () => {
    renderAgent(<AgentPage path={agentPath(RUN)} />);
    await open(t.tab.settings);
    expect(await screen.findByText("A run follows the agent profile builder; its settings are there.", { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: t.openHome })).toHaveAttribute("href", "/projects/helsinki/assistant");
  });

  it("counts a service account's credentials and shows none of them", async () => {
    renderAgent(<AgentPage path={agentPath(ACCOUNT)} />, {
      manifest: () =>
        json({
          apiVersion: "joinedcontext.com/v1",
          kind: "ServiceAccount",
          metadata: { name: "sensor-feed", namespace: "helsinki" },
          spec: { purpose: "feeds the air sensors", credentials: [{ name: "one", secretRef: "s1" }, { name: "two", secretRef: "s2" }] },
        }),
    });
    await open(t.tab.settings);
    const credentials = await screen.findByText(t.setting.credentials);
    expect(credentials.nextElementSibling?.textContent).toBe("2");
    expect(screen.getByText("feeds the air sensors")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/secretRef|s1/);
    expect(screen.getByRole("link", { name: t.openManifest })).toHaveAttribute("href", "/projects/helsinki/settings/service-accounts");
  });

  it("counts none when the manifest holds no credentials", async () => {
    renderAgent(<AgentPage path={agentPath(ACCOUNT)} />, {
      manifest: () => json({ kind: "ServiceAccount", metadata: { name: "sensor-feed" }, spec: { purpose: "p" } }),
    });
    await open(t.tab.settings);
    const credentials = await screen.findByText(t.setting.credentials);
    expect(credentials.nextElementSibling?.textContent).toBe("0");
  });

  it("says why an agent's settings could not be read and reads them after Retry", async () => {
    let healthy = false;
    const asked = renderAgent(<AgentPage path={agentPath(PROFILE)} />, {
      manifest: () =>
        healthy
          ? json({ kind: "AgentProfile", metadata: { name: "builder" }, spec: { role: "builder", model: { name: "m-1" } } })
          : problem(503, "The settings store is restarting."),
    });
    await open(t.tab.settings);
    expect(await screen.findByText(/The settings store is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText("m-1")).toBeInTheDocument();
    expect(asked.filter((path) => path.endsWith("/agentprofiles/builder"))).toHaveLength(2);
  });
});

describe("An agent's page: a list that fails", () => {
  it("says why the agents could not be read and shows the page after Retry", async () => {
    let healthy = false;
    const asked = renderAgent(<AgentPage path={agentPath(MCP)} />, {
      agents: () => (healthy ? json({ items: ITEMS }) : problem(503, "The agent index is restarting.")),
    });
    expect(await screen.findByText(/The agent index is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("heading", { level: 1, name: "geo-tools" })).toBeInTheDocument();
    expect(asked).toHaveLength(2);
  });

  it("says a refusal as it is, with no Retry", async () => {
    renderAgent(<AgentPage path={agentPath(MCP)} />, { agents: () => problem(403, "Your roles read no agent.") });
    expect(await screen.findByText(/Your roles read no agent/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it("names the page's kind and project in its heading line", async () => {
    renderAgent(<AgentPage path={agentPath(MCP)} />);
    const header = (await screen.findByRole("heading", { level: 1, name: "geo-tools" })).parentElement as HTMLElement;
    expect(within(header).getByText(`${t.kind.McpServer} · ${t.organization}`)).toBeInTheDocument();
  });
});
