/**
 * T-5126: Administration → Costs. One month per project with the node share, storage and AI
 * apart, each space and assistant under its project, a partial month said so, the CSV of the
 * same month, and nothing fetched for anybody but an administrator.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { Costs, lastMonth } from "../src/pages/organization/Costs";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const STEWARD = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve"] } }],
} as unknown as Effective;

const GIB = 1024 ** 3;

function costs(month: string, reason?: string) {
  return {
    month,
    currency: "EUR",
    measuredAt: "2026-10-22T01:00:00Z",
    prices: { currency: "EUR", perUsd: 0.9, nodes: [{ name: "cpx42", monthly: 30 }], volumes: [], fixed: [] },
    projects: [
      {
        project: "doprava",
        node: 10,
        storage: 0.5,
        ai: 3.78,
        total: 14.28,
        share: 0.25,
        spaces: [{ space: "cars", bytes: GIB, storage: 0.5 }],
        assistants: [
          { assistant: "help", questions: 120, uncostedQuestions: 2, costUsd: 4.2, cost: 3.78, ...(reason ? { reason } : {}) },
        ],
      },
    ],
    platform: { node: 30, storage: 4.5, total: 34.5 },
    infrastructure: 45,
    ai: 3.78,
    total: 48.78,
  };
}

function renderCosts(permissions: Effective, alone = false, reason?: string, empty = false) {
  const requests: string[] = [];
  const page = alone ? <Costs /> : <OrganizationPage tab="costs" anchor="helsinki" />;
  const result = renderPage(page, {
    path: "/organization/costs",
    answer: (url) => {
      requests.push(url.pathname + url.search);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      const month = url.searchParams.get("month") ?? "";
      if (url.pathname === "/api/v1/organization/costs")
        return json(empty ? { ...costs(month), projects: [] } : costs(month, reason));
      if (url.pathname === "/api/v1/organization/costs/export")
        return new Response(`month,project,part,item,bytes,cost,currency\n${month},doprava,node,,,10.00,EUR\n`, {
          headers: { "content-type": "text/csv" },
        });
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

describe("Administration → Costs", () => {
  it("opens on the last whole month", () => {
    expect(lastMonth(new Date("2026-10-22T01:00:00Z"))).toBe("2026-09");
    expect(lastMonth(new Date("2026-01-05T00:00:00Z"))).toBe("2025-12");
  });

  it("shows each project's parts, its spaces and assistants, and the platform row", async () => {
    const { container } = renderCosts(ADMIN);
    const month = lastMonth(new Date());
    const table = await screen.findByRole("table", { name: `Costs per project, ${month}` });
    await within(table).findByText("doprava");
    const row = within(table).getAllByRole("row")[1];
    expect(within(row).getByText("€10.00")).toBeInTheDocument();
    expect(within(row).getByText("€14.28")).toBeInTheDocument();
    expect(within(row).getByText("Space cars: 1.07 GB, €0.50")).toBeInTheDocument();
    expect(within(row).getByText("Assistant help: 120 questions, $4.20, €3.78")).toBeInTheDocument();
    expect(within(row).getByText(/2 questions had no price/)).toBeInTheDocument();
    expect(within(table).getByText(en.organization.costs.platform)).toBeInTheDocument();
    expect(screen.getByText("Infrastructure €45.00 + AI €3.78 = €48.78")).toBeInTheDocument();
    expect(screen.getByText(en.organization.costs.monthHelp)).toBeInTheDocument();
    expect(screen.queryByText(en.organization.costs.partial)).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("asks for the month picked and downloads its CSV", async () => {
    const created = vi.fn(() => "blob:costs");
    URL.createObjectURL = created;
    URL.revokeObjectURL = vi.fn();
    const { requests } = renderCosts(ADMIN, true);
    await screen.findByText("doprava");
    fireEvent.change(screen.getByLabelText(en.organization.costs.month), { target: { value: "2026-08" } });
    await waitFor(() => expect(requests).toContain("/api/v1/organization/costs?month=2026-08"));
    fireEvent.click(screen.getByRole("button", { name: en.organization.costs.export }));
    await waitFor(() => expect(requests).toContain("/api/v1/organization/costs/export?month=2026-08"));
    await waitFor(() => expect(created).toHaveBeenCalled());
  });

  it("says the month is partial when an assistant did not answer", async () => {
    renderCosts(ADMIN, true, "the assistant answered 503");
    expect(await screen.findByText(en.organization.costs.partial)).toBeInTheDocument();
    expect(screen.getByText("the assistant answered 503")).toBeInTheDocument();
  });

  it("says what to do when no project has a space or an assistant yet", async () => {
    renderCosts(ADMIN, true, undefined, true);
    expect(await screen.findByText(en.organization.costs.empty)).toBeInTheDocument();
  });

  it("tells anybody who is not an administrator who can see it, and fetches nothing", async () => {
    const { requests } = renderCosts(STEWARD, true);
    expect(await screen.findByText(en.organization.costs.hidden)).toBeInTheDocument();
    expect(requests.some((r) => r.startsWith("/api/v1/organization/costs"))).toBe(false);
  });
});
