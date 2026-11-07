/**
 * T-2803: Organization → Health (OPS-53). Every published validation check as a row with its
 * state, last run, counts, trend and failures with their tasks, under one summary line; only an
 * administrator of the organization gets it, and nothing is fetched for anybody else.
 */
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { ValidationHealth } from "../src/pages/organization/ValidationHealth";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

// A steward approves bindings but deletes none: not an administrator (PF-03).
const STEWARD = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve"] } }],
} as unknown as Effective;

const point = (at: string, fail: number) => ({ at, pass: 10, fail, error: 0, skip: 1 });

const CHECKS = [
  {
    check: "authz",
    state: "red",
    result: {
      check: "authz",
      at: "2026-09-25T08:00:00Z",
      everyHours: 24,
      run: "authz 2026-09-25",
      counts: { pass: 130, fail: 2, error: 0, skip: 4 },
      failures: [
        { key: "GET /organization/people/viewer", verdict: "fail", title: "viewer read people", task: "T-2901" },
        { key: "GET /auth/me/anonymous", verdict: "fail", title: "anonymous read the session" },
      ],
      history: [point("2026-09-24T08:00:00Z", 0), point("2026-09-25T08:00:00Z", 2)],
    },
  },
  {
    check: "deployment",
    state: "green",
    result: {
      check: "deployment",
      at: "2026-09-25T09:00:00Z",
      everyHours: 1,
      counts: { pass: 41, fail: 0, error: 0, skip: 3 },
      failures: [],
      history: [point("2026-09-25T08:00:00Z", 0), point("2026-09-25T09:00:00Z", 0)],
    },
  },
  {
    check: "sweep",
    state: "stale",
    result: {
      check: "sweep",
      at: "2026-09-24T09:00:00Z",
      everyHours: 1,
      counts: { pass: 20, fail: 0, error: 0, skip: 0 },
      failures: [],
      history: [],
    },
  },
  { check: "budgets", state: "unreadable" },
];

function renderHealth(permissions: Effective, health: () => Response, alone = false) {
  const requests: string[] = [];
  const page = alone ? <ValidationHealth /> : <OrganizationPage tab="health" anchor="helsinki" />;
  const result = renderPage(page, {
    path: "/organization/health",
    answer: (url) => {
      requests.push(url.pathname);
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      if (url.pathname === "/api/v1/organization/health") return health();
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

describe("Organization → Health", () => {
  it("shows every check with its state, counts, trend and the tasks its failures filed (OPS-53)", async () => {
    const user = userEvent.setup();
    const { container } = renderHealth(ADMIN, () => json({ checks: CHECKS }));
    expect(await screen.findByText("1 check red, 1 check stale, 1 result unreadable")).toBeInTheDocument();
    const table = screen.getByRole("table", { name: en.organization.health.caption });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows.map((row) => within(row).getAllByRole("cell")[1].textContent)).toEqual([
      "Red",
      "Green",
      "Stale",
      "Unreadable",
    ]);
    const authz = rows[0];
    expect(within(authz).getByText("130 passed, 2 failed, 0 errors, 4 skipped")).toBeInTheDocument();
    expect(within(authz).getByRole("img", { name: "2 runs in 7 days, 1 of them red" })).toBeInTheDocument();
    expect(within(authz).getByText("authz 2026-09-25")).toBeInTheDocument();
    await user.click(within(authz).getByText("Show 2 failures"));
    expect(within(authz).getByText("(task T-2901)")).toBeInTheDocument();
    expect(within(authz).getByText("GET /auth/me/anonymous")).toBeInTheDocument();
    expect(within(rows[3]).getByText(en.organization.health.unreadableHint)).toBeInTheDocument();
    // A run with no history draws no line, and a check with no failures offers nothing to open.
    expect(within(rows[2]).queryByRole("img")).toBeNull();
    expect(within(rows[1]).queryByText(/Show/)).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("says everything is green in one line when it is", async () => {
    renderHealth(ADMIN, () => json({ checks: [CHECKS[1]] }));
    expect(await screen.findByText(en.organization.health.summary.green)).toBeInTheDocument();
  });

  it("says nothing has been published yet, rather than an empty table", async () => {
    renderHealth(ADMIN, () => json({ checks: [] }));
    expect(await screen.findByText(en.organization.health.empty)).toBeInTheDocument();
    expect(screen.queryByText(en.organization.health.summary.green)).toBeNull();
  });

  it("shows the API's own words when the results cannot be read", async () => {
    renderHealth(ADMIN, () =>
      json(
        { type: "about:blank", title: "Service Unavailable", status: 503, detail: "the validation results cannot be read" },
        503,
      ),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("the validation results cannot be read");
  });

  it("tells anybody who is not an administrator who can see it, and fetches nothing (OPS-53)", async () => {
    const { requests } = renderHealth(STEWARD, () => json({ checks: CHECKS }), true);
    expect(await screen.findByText(en.organization.health.hidden)).toBeInTheDocument();
    expect(requests).not.toContain("/api/v1/organization/health");
  });
});
