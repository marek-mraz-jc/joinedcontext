/**
 * Administration → Connectors: every column of the grid sorts the way it reads, a failed list
 * says why and Retry asks again, and a revoke that is dismissed or refused changes nothing.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationConnectors } from "../src/pages/organization/OrganizationConnectors";
import { names } from "./grid_checks";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const connector = (id: string, name: string, extra: Record<string, unknown>) => ({
  id: id.repeat(26),
  name,
  owner: "jana-id",
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
  callsPerDay: 1,
  denialsPerDay: 0,
  flagged: false,
  ...extra,
});

const ITEMS = [
  connector("a", "alpha-bot", { segments: ["pipelines"], callsPerDay: 5, lastUsedAt: "2026-10-20T10:00:00Z", allowedClients: ["zeta-client"] }),
  connector("b", "bravo-bot", { segments: ["data-read"], callsPerDay: 40, lastUsedAt: "2026-10-25T10:00:00Z", allowedClients: [] }),
  connector("c", "charlie-bot", { segments: ["mcp"], callsPerDay: 100, lastUsedAt: null, allowedClients: ["alpha-client"] }),
];

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

function renderConnectors(list: () => Response, revoked: () => Response = () => new Response(null, { status: 204 })) {
  const requests: string[] = [];
  renderPage(<OrganizationConnectors />, {
    path: "/organization/connectors",
    answer: (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/connectors" && request.method === "GET") {
        requests.push("GET list");
        return list();
      }
      if (request.method === "DELETE" && url.pathname.startsWith("/api/v1/organization/connectors/")) {
        requests.push(`DELETE ${url.pathname.split("/").pop()}`);
        return revoked();
      }
      return undefined;
    },
  });
  return requests;
}

describe("Administration → Connectors: sorting every column", () => {
  const sorted = async (column: string, clicks: number) => {
    const user = userEvent.setup();
    const table = await screen.findByRole("table", { name: en.organization.connectors.caption });
    await within(table).findByText("alpha-bot");
    for (let i = 0; i < clicks; i += 1) await user.click(within(table).getByRole("button", { name: column }));
    return names(table);
  };
  const c = en.organization.connectors;

  it("sorts by the connector's name", async () => {
    renderConnectors(() => json({ items: ITEMS, denialsFlag: 10 }));
    expect(await sorted(c.connector, 2)).toEqual(["charlie-bot", "bravo-bot", "alpha-bot"]);
  });

  it("sorts by the scope's segments", async () => {
    renderConnectors(() => json({ items: ITEMS, denialsFlag: 10 }));
    // data-read, mcp, pipelines
    expect(await sorted(c.scope, 1)).toEqual(["bravo-bot", "charlie-bot", "alpha-bot"]);
  });

  it("sorts by the clients: a connector open to any client by the words that say so", async () => {
    renderConnectors(() => json({ items: ITEMS, denialsFlag: 10 }));
    const words = [
      ["alpha-bot", "zeta-client"],
      ["bravo-bot", c.anyClient],
      ["charlie-bot", "alpha-client"],
    ].sort((a, b) => a[1].localeCompare(b[1]));
    expect(await sorted(c.clients, 1)).toEqual(words.map((pair) => pair[0]));
  });

  it("sorts by the last call, a connector never used last", async () => {
    renderConnectors(() => json({ items: ITEMS, denialsFlag: 10 }));
    expect(await sorted(c.lastCall, 1)).toEqual(["alpha-bot", "bravo-bot", "charlie-bot"]);
  });

  it("sorts the calls a day as numbers: 5, 40, 100", async () => {
    renderConnectors(() => json({ items: ITEMS, denialsFlag: 10 }));
    expect(await sorted(c.callsPerDay, 1)).toEqual(["alpha-bot", "bravo-bot", "charlie-bot"]);
  });
});

describe("Administration → Connectors: failures and a dismissed revoke", () => {
  const c = en.organization.connectors;

  it("says why the list failed and shows it after Retry", async () => {
    let healthy = false;
    const requests = renderConnectors(() => (healthy ? json({ items: ITEMS, denialsFlag: 10 }) : problem(503, "The connector index is restarting.")));
    expect(await screen.findByText(/The connector index is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText("alpha-bot")).toBeInTheDocument();
    expect(requests.filter((r) => r === "GET list")).toHaveLength(2);
  });

  it("closes the confirmation on Cancel and on Escape without revoking", async () => {
    const user = userEvent.setup();
    const requests = renderConnectors(() => json({ items: ITEMS, denialsFlag: 10 }));
    await user.click(await screen.findByRole("button", { name: "Revoke connector alpha-bot" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Revoke connector alpha-bot?" });
    await user.click(within(dialog).getByRole("button", { name: en.app.cancel }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Revoke connector bravo-bot" }));
    await screen.findByRole("alertdialog", { name: "Revoke connector bravo-bot?" });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(requests.filter((r) => r.startsWith("DELETE"))).toEqual([]);
  });

  it("shows the reason when the revoke is refused and keeps the connector active", async () => {
    const user = userEvent.setup();
    const requests = renderConnectors(
      () => json({ items: ITEMS, denialsFlag: 10 }),
      () => problem(403, "Revoking is for administrators."),
    );
    await user.click(await screen.findByRole("button", { name: "Revoke connector alpha-bot" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Revoke connector alpha-bot?" });
    await user.click(within(dialog).getByRole("button", { name: c.revoke }));
    // The refusal closes the dialog, so its reason is read on the page and not behind the modal.
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const reason = await screen.findByText(/Revoking is for administrators/);
    expect(reason.closest('[role="dialog"], [role="alertdialog"]')).toBeNull();
    expect(requests).toContain(`DELETE ${"a".repeat(26)}`);
    expect(screen.getByRole("button", { name: "Revoke connector alpha-bot" })).toBeInTheDocument();
  });
});
