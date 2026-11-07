/**
 * T-2710: a pipeline's runs and the log of each on the pipelines page (PL-62, ADR-N-034). The
 * row's menu opens them; each run says how many records it wrote, rejected and failed, the newest
 * run's log is open, another run opens by its button, and a log pages older and back. A run whose
 * lines aged out keeps its counts and says so; a pass that wrote nothing is a run of zeros that
 * says so (T-3001); a list that could not be read says why.
 */
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { PipelineRunsDialog, runFacts } from "../src/pages/pipelines/PipelineRuns";
import { names, narrowsAndOrders } from "./grid_checks";
import { expectAxeClean, jsonResponse, list, problem, renderRoute } from "./pageHarness";

const PATH = "/projects/helsinki/pipelines";
const RUNS = "/api/v1/projects/helsinki/pipelines/stations/runs";
const TICK = "2026-09-25T08:15:00Z";
const HOUR = "2026-09-25T07:00Z";

const pipeline = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "stations", namespace: "helsinki" },
  spec: { class: "resident" },
  status: { phase: "Live" },
};

const run = (name: string, sent: number, rejected: number, failed: number) => ({
  run: name,
  firstAt: "2026-09-25T08:15:01Z",
  lastAt: "2026-09-25T08:15:02Z",
  sent,
  rejected,
  failed,
});

const line = (id: number, over: Record<string, unknown> = {}) => ({
  id,
  at: "2026-09-25T08:15:02Z",
  run: TICK,
  recordId: `urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-${id}`,
  outcome: "sent",
  message: "",
  ...over,
});

function answering(runs: unknown, logs: Record<string, Record<string, unknown>>) {
  return (path: string, request: Request): Response | undefined => {
    if (path.endsWith("/pipelines")) {
      return jsonResponse(list([pipeline]));
    }
    if (path === RUNS) {
      return jsonResponse(runs);
    }
    if (path.startsWith(`${RUNS}/`) && path.endsWith("/log")) {
      const name = decodeURIComponent(path.slice(RUNS.length + 1, -"/log".length));
      const before = new URL(request.url).searchParams.get("before") ?? "newest";
      return jsonResponse(logs[name]?.[before] ?? { items: [] });
    }
    return undefined;
  };
}

async function openRuns(): Promise<HTMLElement> {
  await userEvent.click(
    await screen.findByRole("button", { name: en.rowActions.more.replace("{name}", "stations") }),
  );
  await userEvent.click(await screen.findByRole("menuitem", { name: en.pipelines.runs.open }));
  return screen.findByRole("dialog");
}

describe("a pipeline's runs and log", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists_each_run_with_its_counts_and_opens_the_newest_runs_log", async () => {
    const { container } = await renderRoute({
      path: PATH,
      answer: answering(
        { items: [run(TICK, 12, 1, 1), run(HOUR, 40, 0, 0)] },
        {
          [TICK]: {
            newest: {
              items: [
                line(3, { outcome: "failed", step: 1, message: "step 2 threw: not a number" }),
                line(2, { outcome: "rejected", message: "capacity is not an integer" }),
                line(1),
              ],
            },
          },
        },
      ),
    });
    const dialog = await openRuns();
    expect(within(dialog).getByText(en.pipelines.runs.lead)).toBeInTheDocument();
    const runs = await within(dialog).findByRole("table", { name: en.pipelines.runs.caption });
    const rows = within(runs).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(within(rows[1]).getByText("12")).toBeInTheDocument();
    expect(within(rows[2]).getByText(/^Hour from /)).toBeInTheDocument();

    const log = await within(dialog).findByRole("region", { name: /^Log of the run / });
    expect(await within(log).findByText("capacity is not an integer")).toBeInTheDocument();
    // The log's own rows; the grid's Filter panel names the outcomes too.
    const lines = within(log).getByRole("table");
    expect(within(lines).getByText("In step 2")).toBeInTheDocument();
    expect(within(lines).getByText("Ended with an error")).toBeInTheDocument();
    expect(within(lines).getByText("Rejected")).toBeInTheDocument();
    const [newest, older] = within(runs).getAllByRole("button", { name: /^Show the log of / });
    expect(newest).toHaveAttribute("aria-pressed", "true");
    expect(older).toHaveAttribute("aria-pressed", "false");
    await expectAxeClean(container);
  });

  // T-6206: the runs are on the shared grid: search, sort by records sent, filter by outcome.
  it("searches_sorts_by_sent_and_filters_the_runs_by_outcome", async () => {
    const at = (hour: string, sent: number, failed: number) => ({
      run: `2026-09-25T${hour}:00:00Z`,
      firstAt: `2026-09-25T${hour}:00:01Z`,
      lastAt: `2026-09-25T${hour}:00:02Z`,
      sent,
      rejected: 0,
      failed,
    });
    await renderRoute({ path: PATH, answer: answering({ items: [at("10", 5, 0), at("09", 1, 2), at("08", 3, 0)] }, {}) });
    const dialog = await openRuns();
    const table = await within(dialog).findByRole("table", { name: en.pipelines.runs.caption });
    const [ten, nine, eight] = names(table);
    await narrowsAndOrders(table, {
      search: { label: `Search ${en.pipelines.runs.caption}`, text: "T09:", kept: [nine] },
      sort: { column: en.pipelines.runs.sent, ascending: [nine, eight, ten] },
      filter: { value: new RegExp(`^${en.pipelines.runs.states.failing}`), kept: [nine] },
    });
  });

  it("leaves_out_the_message_column_when_no_line_of_the_page_has_one", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({ items: [run(TICK, 2, 0, 0)] }, { [TICK]: { newest: { items: [line(2), line(1)] } } }),
    });
    const dialog = await openRuns();
    const log = await within(dialog).findByRole("region", { name: /^Log of the run / });
    const table = await within(log).findByRole("table");
    // At, record, outcome and the grid's actions column; Message waits under Columns.
    expect(within(table).getAllByRole("columnheader")).toHaveLength(4);
    expect(within(table).queryByRole("columnheader", { name: en.pipelines.runs.message })).toBeNull();
  });

  it("says_how_long_each_run_took_what_moved_since_the_one_before_and_flags_a_silent_run", async () => {
    const at = (name: string, sent: number, first: string, last: string) => ({ run: name, firstAt: first, lastAt: last, sent, rejected: 0, failed: 0 });
    const runs = [
      at("2026-09-25T10:00:00Z", 0, "2026-09-25T10:00:01Z", "2026-09-25T10:00:01Z"),
      at("2026-09-25T09:00:00Z", 40, "2026-09-25T09:00:01Z", "2026-09-25T09:02:31Z"),
      at("2026-09-25T08:00:00Z", 40, "2026-09-25T08:00:01Z", "2026-09-25T08:00:13Z"),
      at("2026-09-25T07:00:00Z", 35, "2026-09-25T07:00:01Z", "2026-09-25T07:00:09Z"),
    ];
    expect(runFacts(runs)).toEqual([
      { seconds: undefined, delta: -40, silent: true },
      { seconds: 150, delta: 0, silent: false },
      { seconds: 12, delta: 5, silent: false },
      { seconds: 8, delta: undefined, silent: false },
    ]);
    // Nothing after nothing is not a stop: the run before wrote nothing either.
    expect(runFacts([runs[0], { ...runs[1], sent: 0 }])[0].silent).toBe(false);

    await renderRoute({ path: PATH, answer: answering({ items: runs }, {}) });
    const dialog = await openRuns();
    expect(await within(dialog).findByRole("status")).toHaveTextContent("The last run wrote nothing, while the run before wrote 40.");
    const rows = within(within(dialog).getByRole("table", { name: en.pipelines.runs.caption })).getAllByRole("row");
    expect(rows[1]).toHaveTextContent(en.pipelines.runs.silentBadge);
    expect(rows[2]).toHaveTextContent("3 minutes");
    expect(rows[2]).toHaveTextContent(en.pipelines.runs.same);
    expect(rows[3]).toHaveTextContent("12 seconds");
    expect(rows[3]).toHaveTextContent("+5");
  });

  it("says what a run's written records did, and nothing for a run reported without hashes (T-3304)", async () => {
    const runs = [
      { ...run(TICK, 12, 0, 0), created: 3, updated: 2, unchanged: 7 },
      { ...run(HOUR, 40, 0, 0), created: null, updated: null, unchanged: null },
    ];
    await renderRoute({ path: PATH, answer: answering({ items: runs }, {}) });
    const dialog = await openRuns();
    const rows = within(await within(dialog).findByRole("table", { name: en.pipelines.runs.caption })).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("3 new, 2 changed, 7 unchanged");
    expect(rows[2]).not.toHaveTextContent("new,");
  });

  it("opens_another_run_and_pages_its_log_older_and_back", async () => {
    await renderRoute({
      path: PATH,
      answer: answering(
        { items: [run(TICK, 1, 0, 0), run(HOUR, 3, 0, 0)] },
        {
          [TICK]: { newest: { items: [line(9)] } },
          [HOUR]: {
            newest: { items: [line(8, { recordId: "urn:hour-8" })], next: 8 },
            "8": { items: [line(4, { recordId: "urn:hour-4" })] },
          },
        },
      ),
    });
    const dialog = await openRuns();
    await within(dialog).findByText("urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-9");
    // The log shows the entity's own name; the whole URN is the hover title (T-4991).
    expect(within(dialog).getByText("s-9")).toHaveAttribute("aria-hidden");
    expect(within(dialog).getByTitle("urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-9")).toBeInTheDocument();
    const runs = within(dialog).getByRole("table", { name: en.pipelines.runs.caption });
    await userEvent.click(within(runs).getAllByRole("button", { name: /^Show the log of / })[1]);
    expect(await within(dialog).findByText("urn:hour-8")).toBeInTheDocument();
    expect(within(dialog).queryByText("urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-9")).toBeNull();

    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.runs.older }));
    expect(await within(dialog).findByText("urn:hour-4")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: en.pipelines.runs.older })).toBeNull();
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.runs.newest }));
    expect(await within(dialog).findByText("urn:hour-8")).toBeInTheDocument();
  });

  it("no_runs_says_when_the_first_comes_and_aged_out_lines_keep_the_counts", async () => {
    await renderRoute({ path: PATH, answer: answering({ items: [] }, {}) });
    const empty = await openRuns();
    expect(await within(empty).findByText(en.pipelines.runs.empty)).toBeInTheDocument();
    expect(within(empty).queryByRole("table")).toBeNull();
  });

  it("a_run_whose_lines_aged_out_keeps_its_counts_and_says_so", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({ items: [run(TICK, 4000, 0, 0)] }, {}),
    });
    const dialog = await openRuns();
    expect(await within(dialog).findByText(en.pipelines.runs.noLines)).toBeInTheDocument();
    expect(within(dialog).getByText("4,000")).toBeInTheDocument();
  });

  // T-3001: a pass that read its source and found nothing new is a run of zeros, and its log
  // says so rather than claiming lines that aged out.
  it("a_pass_that_wrote_nothing_is_a_run_of_zeros_and_says_it_found_nothing", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({ items: [run(TICK, 0, 0, 0), run(HOUR, 40, 0, 0)] }, {}),
    });
    const dialog = await openRuns();
    expect(await within(dialog).findByText(en.pipelines.runs.nothingWritten)).toBeInTheDocument();
    expect(within(dialog).queryByText(en.pipelines.runs.noLines)).toBeNull();
    const rows = within(within(dialog).getByRole("table", { name: en.pipelines.runs.caption })).getAllByRole("row");
    expect(within(rows[1]).getAllByText("0")).toHaveLength(3);
  });

  it("runs_that_could_not_be_read_say_why_and_escape_closes", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = new URL(input instanceof Request ? input.url : String(input), window.location.origin)
          .pathname;
        return Promise.resolve(
          path.endsWith("/permissions/me")
            ? jsonResponse({ project: "helsinki", bootstrap: true, grants: [] })
            : problem(503, "The pipeline's runs could not be read; try again."),
        );
      }),
    );
    const onClose = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PipelineRunsDialog project="helsinki" name="stations" onClose={onClose} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "The pipeline's runs could not be read; try again.",
    );
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  /** The dialog alone, against `answer` for every request but the caller's permissions. */
  function dialogOn(
    answer: (path: string, request: Request) => Response,
    grants: unknown[] | "all",
    runsFromQueue: boolean,
  ) {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
        const path = new URL(request.url, window.location.origin).pathname;
        if (path.endsWith("/permissions/me")) {
          return Promise.resolve(
            jsonResponse(
              grants === "all"
                ? { project: "helsinki", bootstrap: true, grants: [] }
                : { project: "helsinki", bootstrap: false, grants },
            ),
          );
        }
        return Promise.resolve(answer(path, request));
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PipelineRunsDialog project="helsinki" name="stations" onClose={() => {}} runsFromQueue={runsFromQueue} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return screen.findByRole("dialog");
  }

  it("a_steward_runs_a_queued_pipeline_now_and_is_told_which_run_was_queued", async () => {
    const asked: string[] = [];
    const dialog = await dialogOn(
      (path, request) => {
        asked.push(`${request.method} ${path}`);
        return request.method === "POST"
          ? jsonResponse({ run: "2026-10-16T14:00:00Z" }, 202)
          : jsonResponse({ items: [] });
      },
      "all",
      true,
    );
    await userEvent.click(await within(dialog).findByRole("button", { name: en.pipelines.runs.runNow }));
    expect(await within(dialog).findByText(/^Run .+ is queued\. It starts when its turn comes/)).toBeInTheDocument();
    expect(asked).toContain(`POST ${RUNS}`);
  });

  it("run_now_is_offered_only_for_a_queued_pipeline_needs_approve_and_shows_a_refusal", async () => {
    const empty = () => jsonResponse({ items: [] });
    const resident = await dialogOn(empty, "all", false);
    await within(resident).findByText(en.pipelines.runs.empty);
    expect(within(resident).queryByRole("button", { name: en.pipelines.runs.runNow })).toBeNull();
    cleanup();

    const editor = await dialogOn(empty, [{ rule: { kinds: ["Pipeline"], verbs: ["read", "propose"] } }], true);
    expect(await within(editor).findByRole("button", { name: en.pipelines.runs.runNow })).toHaveAttribute("aria-disabled", "true");
    cleanup();

    const busy = await dialogOn(
      (_, request) =>
        request.method === "POST"
          ? problem(409, "pipeline 'stations' is running (2026-10-16T05:00:00Z); start another once it ended")
          : jsonResponse({ items: [] }),
      "all",
      true,
    );
    await userEvent.click(await within(busy).findByRole("button", { name: en.pipelines.runs.runNow }));
    expect(await within(busy).findByRole("alert")).toHaveTextContent("is running (2026-10-16T05:00:00Z)");
  });
});
