/**
 * T-5975: Administration → Connectors. An administrator sees every person's connectors with
 * scope, clients and the last day's usage, filters them, finds a flagged one and revokes it after
 * a confirmation; anybody else is told the page is the administrator's and nothing is fetched.
 */
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { names } from "./grid_checks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationConnectors } from "../src/pages/organization/OrganizationConnectors";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const VIEWER = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "viewer", binding: "viewers", rule: { kinds: ["*"], verbs: ["read"] } }],
} as unknown as Effective;

const connector = (id: string, name: string, owner: string, extra: Record<string, unknown> = {}) => ({
  id,
  name,
  owner,
  segments: ["data-read"],
  scope: [{ project: "doprava", endpoint: "counters", access: "read" }],
  writes: false,
  expiresAt: "2026-11-27T00:00:00Z",
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  state: "active",
  createdAt: "2026-10-27T09:00:00Z",
  lastUsedAt: "2026-10-28T10:00:00Z",
  url: `https://portal.test/c/${id}`,
  limits: { resultBytes: 1048576 },
  callsPerDay: 4,
  denialsPerDay: 0,
  flagged: false,
  ...extra,
});

function renderConnectors(permissions: Effective, alone = false) {
  const requests: string[] = [];
  let items = [
    connector("a".repeat(26), "jana-claude-code", "jana-id", { callsPerDay: 31, denialsPerDay: 12, flagged: true }),
    connector("b".repeat(26), "pia-chatgpt", "pia-id", { allowedClients: ["https://chatgpt.com/oauth/metadata"] }),
    connector("c".repeat(26), "ola-old", "ola-id", { state: "revoked", lastUsedAt: null }),
  ];
  const page = alone ? <OrganizationConnectors /> : <OrganizationPage tab="connectors" anchor="helsinki" />;
  const result = renderPage(page, {
    path: "/organization/connectors",
    answer: (url, request) => {
      requests.push(`${request.method} ${url.pathname}`);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/connectors") return json({ items, denialsFlag: 10 });
      if (request.method === "DELETE" && url.pathname.startsWith("/api/v1/organization/connectors/")) {
        const id = url.pathname.split("/").pop();
        items = items.map((item) => (item.id === id ? { ...item, state: "revoked" } : item));
        return new Response(null, { status: 204 });
      }
      return undefined;
    },
  });
  return { ...result, requests };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Administration → Connectors", () => {
  it("lists, flags, filters and revokes a person's connector after a confirmation", async () => {
    const user = userEvent.setup();
    const { container, requests } = renderConnectors(ADMIN);
    const table = await screen.findByRole("table", { name: en.organization.connectors.caption });
    expect(await within(table).findByText("jana-claude-code")).toBeInTheDocument();
    expect(screen.getByText(/^1 active connector is flagged: 10 or more refused calls/)).toBeInTheDocument();
    // Active only at first: the revoked connector waits behind the state filter.
    expect(within(table).queryByText("ola-old")).toBeNull();
    const jana = within(table).getByText("jana-claude-code").closest("tr") as HTMLElement;
    expect(within(jana).getByText("31 calls")).toBeInTheDocument();
    expect(within(jana).getByText("12 refused")).toBeInTheDocument();
    expect(within(jana).getByText(en.organization.connectors.flagged)).toBeInTheDocument();
    expect(within(jana).getByText("doprava/counters")).toBeInTheDocument();
    expect(container.textContent).not.toMatch(/token|secret/i);

    await user.type(screen.getByLabelText(en.organization.connectors.filter), "chatgpt");
    expect(within(table).queryByText("jana-claude-code")).toBeNull();
    expect(within(table).getByText("pia-chatgpt")).toBeInTheDocument();
    await user.clear(screen.getByLabelText(en.organization.connectors.filter));
    await user.click(screen.getByLabelText(en.organization.connectors.flaggedOnly, { exact: false }));
    expect(within(table).queryByText("pia-chatgpt")).toBeNull();

    await user.click(within(table).getByRole("button", { name: "Revoke connector jana-claude-code" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Revoke connector jana-claude-code?" });
    await user.click(within(dialog).getByRole("button", { name: en.organization.connectors.revoke }));
    expect(await screen.findByText(en.organization.connectors.noMatch)).toBeInTheDocument();
    expect(requests).toContain(`DELETE /api/v1/organization/connectors/${"a".repeat(26)}`);

    await user.click(screen.getByLabelText(en.organization.connectors.flaggedOnly, { exact: false }));
    await user.selectOptions(screen.getByLabelText(en.organization.connectors.state), "revoked");
    // The grid drew its empty card while nothing matched: the rows come back in a table of their own.
    const revoked = await screen.findByRole("table", { name: en.organization.connectors.caption });
    expect(await within(revoked).findByText("jana-claude-code")).toBeInTheDocument();
    expect(within(revoked).getByText("ola-old")).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("tells anybody else the page is the administrator's and fetches nothing", async () => {
    const { requests } = renderConnectors(VIEWER);
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(requests.filter((r) => r.includes("/organization/connectors"))).toEqual([]);
    cleanup();
    // The section alone, as another page may embed it, says the same in its own words.
    const alone = renderConnectors(VIEWER, true);
    expect(await screen.findByText(en.organization.connectors.hidden)).toBeInTheDocument();
    expect(alone.requests.filter((r) => r.includes("/organization/connectors"))).toEqual([]);
  });

  it("sorts by refusals and filters by owner on the shared grid (T-6205)", async () => {
    const user = userEvent.setup();
    renderConnectors(ADMIN);
    const table = await screen.findByRole("table", { name: en.organization.connectors.caption });
    await within(table).findByText("jana-claude-code");
    const sort = within(table).getByRole("button", { name: `Sort by ${en.organization.connectors.denialsPerDay}` });
    await user.click(sort);
    expect(names(table)).toEqual(["pia-chatgpt", "jana-claude-code"]);
    await user.click(sort);
    expect(names(table)).toEqual(["jana-claude-code", "pia-chatgpt"]);
    await user.click(within(table.closest<HTMLElement>("[data-manifest-grid]")!).getByText(en.grid.filter));
    await user.click(screen.getByRole("checkbox", { name: /^pia/i }));
    expect(names(table)).toEqual(["pia-chatgpt"]);
  });
});
