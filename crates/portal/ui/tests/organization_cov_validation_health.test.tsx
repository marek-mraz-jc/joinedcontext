/**
 * Organization → Health: the grid sorts on the failures and the counts a check published, and a
 * check that published no failure sorts apart from the one that did.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { ValidationHealth } from "../src/pages/organization/ValidationHealth";
import { json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const CHECKS = [
  {
    check: "sweep",
    state: "green",
    result: { check: "sweep", at: "2026-09-25T09:00:00Z", counts: { pass: 20, fail: 0, error: 0, skip: 0 }, failures: [], history: [] },
  },
  {
    check: "authz",
    state: "red",
    result: {
      check: "authz",
      at: "2026-09-25T08:00:00Z",
      counts: { pass: 130, fail: 1, error: 0, skip: 0 },
      failures: [{ key: "GET /people", verdict: "fail", title: "viewer read people" }],
      history: [],
    },
  },
  {
    check: "restore",
    state: "red",
    result: {
      check: "restore",
      at: "2026-09-25T07:00:00Z",
      counts: { pass: 3, fail: 1, error: 0, skip: 0 },
      failures: [{ key: "ZZ drill", verdict: "fail", title: "restore drill" }],
      history: [],
    },
  },
  { check: "budgets", state: "unreadable" },
];

const checkNames = (table: HTMLElement) =>
  within(table)
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.textContent ?? "");

const sortBy = (label: string, item: string) => {
  fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${label}` }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: item }));
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  try {
    window.localStorage.clear();
  } catch {
    // no storage in this run
  }
});
afterEach(cleanup);

function renderHealth() {
  return renderPage(<ValidationHealth />, {
    path: "/organization/health",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/health") return json({ checks: CHECKS });
      return undefined;
    },
  });
}

describe("Organization → Health: sorting what a check published", () => {
  it("orders the checks by the failures they published, and keeps a check without any last", async () => {
    renderHealth();
    await screen.findByText("authz");
    const table = await screen.findByRole("table", { name: en.organization.health.caption });
    sortBy(en.organization.health.failures, en.entityGrid.sortAsc);
    let rows = checkNames(table);
    expect(rows[0]).toContain("authz");
    expect(rows[1]).toContain("restore");
    sortBy(en.organization.health.failures, en.entityGrid.sortDesc);
    rows = checkNames(table);
    expect(rows[0]).toContain("restore");
    expect(rows[1]).toContain("authz");
    expect(rows[2] + rows[3]).toMatch(/sweep|budgets/);
  });

  it("sorts by the counts, and a result nobody could read sorts as a dash", async () => {
    renderHealth();
    await screen.findByText("authz");
    const table = await screen.findByRole("table", { name: en.organization.health.caption });
    sortBy(en.organization.health.results, en.entityGrid.sortAsc);
    // The counts are compared as the text a person reads ("3 passed" after "20 passed").
    expect(checkNames(table)[0]).toContain("budgets");
    expect(checkNames(table)[3]).toContain("restore");
    sortBy(en.organization.health.results, en.entityGrid.sortDesc);
    expect(checkNames(table)[0]).toContain("restore");
  });
});
