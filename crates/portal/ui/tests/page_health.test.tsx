/**
 * T-5067: the project's Health page and the administrator's view across projects. Errors come
 * first with what was found and how to fix it, the kind filter narrows the list, an empty log and
 * a refusal say what next.
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import type { components } from "../src/api/schema";
import { CHECKS as CHECK_NAMES, LABEL, SAID } from "../src/pages/health/HealthLog";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

/** Every server kind once: a kind added to the API without a key here is a type error. */
const KINDS = Object.keys({ pipeline: 1, space: 1, endpoint: 1, model: 1, datasource: 1 } satisfies Record<components["schemas"]["HealthCheckKind"], 1>) as components["schemas"]["HealthCheckKind"][];

const none = { code: "", args: {}, text: "" };
const said = (code: string, text: string, args: Record<string, string> = {}) => ({ code, args, text });

const check = (over: Record<string, unknown>) => ({
  project: "bbsk",
  kind: "pipeline",
  name: "podujatia-zvolen",
  check: "runs",
  status: "ok",
  since: "2026-10-21T06:00:00Z",
  lastOk: "2026-10-21T08:00:00Z",
  checkedAt: "2026-10-21T08:00:00Z",
  detail: none,
  fix: none,
  evidenceUrl: "/projects/bbsk/pipelines/podujatia-zvolen",
  history: [
    { at: "2026-10-20T06:00:00Z", status: "ok" },
    { at: "2026-10-21T06:00:00Z", status: "error" },
  ],
  ...over,
});

const CHECKS = [
  check({
    status: "error",
    lastOk: undefined,
    detail: said("", "3 runs in a row sent nothing: 403 no policy grants this operation"),
    fix: said("policyWrite", "The broker refused the write (English from the server).", { name: "podujatia-zvolen" }),
  }),
  check({ kind: "endpoint", name: "air-api", check: "reconcile", status: "warn", detail: said("waitingApproval", "waiting for approval"), evidenceUrl: "/projects/bbsk/endpoints/air-api" }),
  check({ kind: "space", name: "air", check: "reconcile", evidenceUrl: "/projects/bbsk/spaces/air", history: [] }),
];

const answering = (health: () => Response) => (path: string): Response | undefined =>
  path.endsWith("/health-checks") ? health() : undefined;

describe("the health log", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists_errors_first_with_the_fix_and_filters_by_kind", async () => {
    const { container, calls } = await renderRoute({
      path: "/projects/bbsk/health",
      answer: answering(() => jsonResponse({ items: CHECKS })),
    });
    await screen.findByRole("columnheader", { name: en.health.found });
    const table = screen.getByRole("table", { name: en.health.caption });
    const rows = within(table).getAllByRole("row").slice(1);
    // The list opens on what needs attention: the working check waits behind the status filter.
    expect(rows).toHaveLength(2);
    expect(screen.getByRole("combobox", { name: en.health.statusFilter })).toHaveValue("attention");
    expect(within(rows[0]).getByText(en.health.broken)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/403 no policy grants/)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/pl-podujatia-zvolen create and update/)).toBeInTheDocument();
    expect(within(rows[0]).queryByText(/English from the server/)).toBeNull();
    expect(within(rows[0]).getByRole("link", { name: "podujatia-zvolen" })).toHaveAttribute(
      "href",
      "/projects/bbsk/pipelines/podujatia-zvolen",
    );
    expect(within(rows[0]).getByRole("img", { name: "2 readings in the last 7 days: 1 errors, 0 warnings." })).toBeInTheDocument();
    expect(within(rows[0]).getByText(en.health.never)).toBeInTheDocument();
    expect(screen.getByText("1 error and 1 warning among 3 checks: read the fix under each.")).toBeInTheDocument();
    expect(calls()).toContain("GET /api/v1/projects/bbsk/health-checks");
    await expectAxeClean(container);

    await userEvent.selectOptions(screen.getByRole("combobox", { name: en.health.statusFilter }), "all");
    expect(within(screen.getByRole("table", { name: en.health.caption })).getAllByRole("row").slice(1)).toHaveLength(3);
    expect(screen.getByRole("option", { name: "Working (1)" })).toBeInTheDocument();

    // T-6206: the kind is the shared grid's filter.
    await userEvent.click(screen.getByText(en.grid.filter));
    await userEvent.click(screen.getByRole("checkbox", { name: new RegExp(`^${en.health.kinds.endpoint}`) }));
    const filtered = within(screen.getByRole("table", { name: en.health.caption })).getAllByRole("row").slice(1);
    expect(filtered).toHaveLength(1);
    expect(within(filtered[0]).getByText("air-api")).toBeInTheDocument();

    await userEvent.selectOptions(screen.getByRole("combobox", { name: en.health.statusFilter }), "ok");
    expect(await screen.findByText(en.grid.noMatch)).toBeInTheDocument();
  });

  it("says_what_to_do_when_nothing_was_checked", async () => {
    await renderRoute({ path: "/projects/bbsk/health", answer: answering(() => jsonResponse({ items: [] })) });
    expect(await screen.findByText(en.health.empty)).toBeInTheDocument();
    expect(screen.getByText(en.health.emptyHint)).toBeInTheDocument();
  });

  it("says_all_is_well_when_every_check_is_ok", async () => {
    await renderRoute({
      path: "/projects/bbsk/health",
      answer: answering(() => jsonResponse({ items: [CHECKS[2]] })),
    });
    expect(await screen.findByText("All 1 checks are ok.")).toBeInTheDocument();
  });

  it("shows_the_server_s_refusal_in_words", async () => {
    const said = "project 'bbsk' not found";
    await renderRoute({ path: "/projects/bbsk/health", answer: answering(() => problem(404, said)) });
    expect(await screen.findByRole("alert")).toHaveTextContent(said);
  });

  it("gives_the_administrator_every_project_with_a_project_column", async () => {
    const { calls } = await renderRoute({
      path: "/organization/health",
      answer: (path: string) =>
        path.endsWith("/organization/health-checks")
          ? jsonResponse({ items: [CHECKS[0], { ...CHECKS[1], project: "helsinki" }] })
          : path.endsWith("/organization/health")
            ? jsonResponse({ checks: [] })
            : undefined,
    });
    await screen.findByRole("columnheader", { name: en.health.found });
    const table = screen.getByRole("table", { name: en.health.caption });
    expect(within(table).getByText(`${en.health.project}: helsinki`)).toBeInTheDocument();
    expect(calls()).toContain("GET /api/v1/organization/health-checks");
  });

  it("names_every_kind_check_and_status_in_every_locale", () => {
    for (const locale of [en, sk, cs, de]) {
      for (const kind of KINDS) expect(locale.health.kinds[kind]).toBeTruthy();
      for (const name of CHECK_NAMES) expect(locale.health.checks[name]).toBeTruthy();
      for (const key of Object.values(LABEL)) expect(locale.health[key.replace("health.", "") as "broken"]).toBeTruthy();
    }
  });

  it("speaks_the_server_s_messages_in_slovak_and_keeps_a_component_s_own_words", async () => {
    await i18n.changeLanguage("sk");
    await renderRoute({
      path: "/projects/bbsk/health",
      locale: "sk",
      answer: answering(() => jsonResponse({ items: CHECKS })),
    });
    await screen.findByRole("columnheader", { name: sk.health.found });
    const rows = within(screen.getByRole("table", { name: sk.health.caption })).getAllByRole("row").slice(1);
    expect(within(rows[0]).getByText(/Policy cieľového Endpointu musí dať pl-podujatia-zvolen/)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/403 no policy grants/)).toBeInTheDocument();
    expect(within(rows[1]).getByText(sk.health.say.waitingApproval)).toBeInTheDocument();
  });

  // T-6206: the shared grid stacks each row as a card on a phone (Table stack), the whole finding in it.
  it("gives_a_phone_one_card_per_check_with_the_whole_finding", async () => {
    await renderRoute({ path: "/projects/bbsk/health", answer: answering(() => jsonResponse({ items: CHECKS })) });
    await screen.findByRole("columnheader", { name: en.health.found });
    const cards = within(screen.getByRole("table", { name: en.health.caption })).getAllByRole("row").slice(1);
    expect(cards).toHaveLength(2);
    expect(within(cards[0]).getByText(/403 no policy grants/)).toBeInTheDocument();
    expect(within(cards[0]).getByText(/pl-podujatia-zvolen create and update/)).toBeInTheDocument();
  });

  it("names_every_message_the_server_sends_in_every_locale", () => {
    for (const locale of [en, sk, cs, de]) {
      for (const code of SAID) expect(locale.health.say[code], code).toBeTruthy();
    }
  });
});
