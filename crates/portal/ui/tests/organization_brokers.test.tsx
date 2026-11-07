/**
 * T-3990: Organization → Brokers (PF-116, API/01 §43). Every broker with its health, version,
 * spaces and load; a broker that does not answer says so with its reason; an organization that
 * declares none shows the built-in main; only an administrator gets it.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { Brokers } from "../src/pages/organization/Brokers";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";
import { narrowsAndOrders } from "./grid_checks";

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

const MAIN = {
  name: "main",
  declared: true,
  default: true,
  size: "small",
  features: ["ogc"],
  health: "up",
  version: "0.14.2",
  reason: null,
  spaces: ["ovzdusie/air", "doprava/stops"],
  entities: 14232,
  observedAt: "2026-10-12T08:00:00Z",
};

const CANARY = {
  name: "canary",
  declared: true,
  default: false,
  size: "small",
  features: [],
  health: "down",
  version: null,
  reason: "the broker did not answer; try again in a minute",
  spaces: [],
  entities: 0,
  observedAt: "2026-10-12T08:00:00Z",
};

function renderBrokers(permissions: Effective, items: unknown[], alone = false) {
  const requests: string[] = [];
  const page = alone ? <Brokers /> : <OrganizationPage tab="brokers" anchor="helsinki" />;
  const result = renderPage(page, {
    path: "/organization/brokers",
    answer: (url) => {
      requests.push(url.pathname);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/brokers") return json({ items });
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

describe("Organization → Brokers", () => {
  it("shows each healthy broker with its version, spaces and load", async () => {
    const { container } = renderBrokers(ADMIN, [MAIN]);
    await screen.findByText("0.14.2");
    const table = screen.getByRole("table", { name: en.organization.brokers.caption });
    const row = within(table).getAllByRole("row")[1];
    expect(within(row).getByText("Up")).toBeInTheDocument();
    expect(within(row).getByText("0.14.2")).toBeInTheDocument();
    expect(within(row).getByText("ovzdusie/air")).toBeInTheDocument();
    expect(within(row).getByText("14,232")).toBeInTheDocument();
    expect(within(row).getByText(en.organization.brokers.default)).toBeInTheDocument();
    expect(screen.queryByText(/not answering/)).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("says a broker is down with its reason and shows no load for it", async () => {
    renderBrokers(ADMIN, [CANARY, MAIN]);
    expect(
      await screen.findByText("1 broker is not answering. Check its pod and the reason in its row."),
    ).toBeInTheDocument();
    const table = screen.getByRole("table", { name: en.organization.brokers.caption });
    const canary = within(table).getAllByRole("row")[1];
    expect(within(canary).getByText("Down")).toBeInTheDocument();
    expect(within(canary).getByText(CANARY.reason)).toBeInTheDocument();
    expect(within(canary).getByText(en.organization.brokers.noSpaces)).toBeInTheDocument();
  });

  it("explains the built-in main when the organization declares no broker", async () => {
    renderBrokers(ADMIN, [{ ...MAIN, declared: false, size: undefined, features: [] }]);
    expect(await screen.findByText(en.organization.brokers.implicit)).toBeInTheDocument();
  });

  it("tells anybody who is not an administrator who can see it, and fetches nothing", async () => {
    const { requests } = renderBrokers(STEWARD, [MAIN], true);
    expect(await screen.findByText(en.organization.brokers.hidden)).toBeInTheDocument();
    expect(requests).not.toContain("/api/v1/organization/brokers");
  });

  it("searches, sorts by entities and filters by health on the shared grid (T-6205)", async () => {
    const EDGE = { ...MAIN, name: "edge", default: false, version: "0.14.1", spaces: ["parking/lots"], entities: 100 };
    renderBrokers(ADMIN, [MAIN, CANARY, EDGE]);
    await screen.findByText("0.14.1");
    const table = screen.getByRole("table", { name: en.organization.brokers.caption });
    await narrowsAndOrders(table, {
      search: { label: "Search Context brokers", text: "parking", kept: ["edge"] },
      sort: { column: en.organization.brokers.entities, ascending: ["canary", "edge", "main"] },
      filter: { value: /^Up/, kept: ["main", "edge"] },
    });
  });
});
