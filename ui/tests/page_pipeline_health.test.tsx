/**
 * T-5048: the pipelines page's Source health tab. A steward reads each source's state, its last
 * delivery, the change against its 7-day median and its last error, worst first, and opens the
 * runs of one; a reader is never offered the tab, and a refusal or an empty project says what next.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { byUrgency } from "../src/pages/pipelines/PipelineHealth";
import { expectAxeClean, jsonResponse, list, problem, renderRoute } from "./pageHarness";

const PATH = "/projects/helsinki/pipelines";

const pipeline = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: "helsinki" },
  spec: { class: "scheduled" },
  status: { phase: "Live" },
});

const HEALTH = [
  {
    pipeline: "podujatia-detva",
    state: "ok",
    lastSuccessAt: "2026-10-21T06:00:00Z",
    lastRunAt: "2026-10-21T06:00:00Z",
    lastSent: 41,
    medianSent: 40,
    changePercent: 3,
    failedRuns: 0,
    alert: false,
    links: { checkedAt: "2026-10-19T02:00:00Z", checked: 40, answered: 39, skipped: 0, deadCount: 0, dead: [] },
  },
  {
    pipeline: "podujatia-zvolen",
    state: "failing",
    lastSuccessAt: "2026-10-18T06:00:00Z",
    lastRunAt: "2026-10-21T06:00:00Z",
    lastSent: 0,
    medianSent: 12,
    changePercent: -100,
    failedRuns: 3,
    lastError: "GET https://zvolen.example/rss answered 404",
    lastErrorAt: "2026-10-21T06:00:01Z",
    alert: true,
    links: {
      checkedAt: "2026-10-19T02:00:00Z",
      checked: 8,
      answered: 1,
      skipped: 2,
      deadCount: 7,
      dead: [
        ...[1, 2, 3, 4].map((n) => ({ url: `https://zvolen.example/podujatia/${n}`, status: 404 as number | undefined })),
        { url: "https://zvolen.example/podujatia/5", status: undefined, error: "no answer in 10 s" },
      ],
    },
  },
  { pipeline: "podujatia-revuca", state: "empty", lastRunAt: "2026-10-21T06:00:00Z", lastSent: 0, failedRuns: 0, alert: false },
  { pipeline: "podujatia-new", state: "never", failedRuns: 0, alert: false },
];

const answering =
  (health: () => Response) =>
  (path: string): Response | undefined => {
    if (path.endsWith("/pipeline-health")) return health();
    if (path.endsWith("/pipelines")) return jsonResponse(list(HEALTH.map((h) => pipeline(h.pipeline))));
    if (path.endsWith("/runs")) return jsonResponse({ items: [] });
    return undefined;
  };

async function openHealthTab(): Promise<void> {
  await userEvent.click(await screen.findByRole("tab", { name: en.pipelines.map.view.health }));
}

describe("the source health tab", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists_every_source_worst_first_with_its_error_and_opens_its_runs", async () => {
    const { container, calls } = await renderRoute({
      path: PATH,
      answer: answering(() => jsonResponse({ items: HEALTH })),
    });
    await openHealthTab();
    const table = await screen.findByRole("table", { name: en.pipelines.health.caption });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows.map((row) => within(row).getAllByRole("cell")[0].textContent)).toEqual([
      "podujatia-zvolen",
      "podujatia-revuca",
      "podujatia-new",
      "podujatia-detva",
    ]);
    expect(within(rows[0]).getByText(en.pipelines.health.states.failing)).toBeInTheDocument();
    expect(within(rows[0]).getByText("3 failed runs in a row")).toBeInTheDocument();
    expect(within(rows[0]).getByText(en.pipelines.health.alerted)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/zvolen\.example\/rss answered 404/)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/100 %/)).toBeInTheDocument();
    expect(within(rows[2]).getByText(en.pipelines.health.states.never)).toBeInTheDocument();
    expect(screen.getByText("1 of 4 sources needs a look; 1 is alerted.")).toBeInTheDocument();
    expect(window.location.search).toBe("?view=health");
    await expectAxeClean(container);

    await userEvent.click(within(rows[0]).getByRole("button", { name: "Runs and log of podujatia-zvolen" }));
    expect(await screen.findByRole("dialog", { name: "Runs of podujatia-zvolen" })).toBeInTheDocument();
    await waitFor(() =>
      expect(calls()).toContain("GET /api/v1/projects/helsinki/pipelines/podujatia-zvolen/runs"),
    );
  });

  it("flags_a_source_with_dead_links_and_says_how_to_fix_it", async () => {
    const { container } = await renderRoute({
      path: PATH,
      answer: answering(() => jsonResponse({ items: HEALTH })),
    });
    await openHealthTab();
    const table = await screen.findByRole("table", { name: en.pipelines.health.caption });
    const rows = within(table).getAllByRole("row").slice(1);
    const zvolen = rows[0];
    expect(within(zvolen).getByText("1 of 8 answer (12 %)")).toHaveClass("text-danger");
    expect(within(zvolen).getByText("Dead links: 7")).toBeInTheDocument();
    expect(within(zvolen).getAllByRole("listitem")).toHaveLength(5);
    expect(within(zvolen).getByTitle("https://zvolen.example/podujatia/1").closest("li")).toHaveTextContent("(answered 404)");
    expect(within(zvolen).getByTitle("https://zvolen.example/podujatia/5").closest("li")).toHaveTextContent("(no answer)");
    expect(within(zvolen).getByText("and 2 more")).toBeInTheDocument();
    expect(within(zvolen).getByText(en.pipelines.health.linksFix)).toBeInTheDocument();
    const detva = rows[3];
    expect(within(detva).getByText("39 of 40 answer (97 %)")).not.toHaveClass("text-danger");
    expect(within(detva).queryByText(en.pipelines.health.linksFix)).toBeNull();
    expect(within(rows[2]).getByText(en.pipelines.health.linksNotChecked)).toBeInTheDocument();
    await expectAxeClean(container);
  });

  // T-5291: a stream that keeps sending but loses a tenth of its records is red, with the ratio.
  it("shows_a_running_source_that_loses_records_as_failing_with_its_error_ratio", async () => {
    const losing = [
      {
        pipeline: "hsl-hfp-vehicles",
        state: "failing",
        lastSuccessAt: "2026-10-22T18:00:00Z",
        lastRunAt: "2026-10-22T18:00:00Z",
        lastSent: 900,
        failedRuns: 0,
        lastError: "step 2 threw: speed is not a number",
        errors: { errors: 100, records: 1000, percent: 10 },
        alert: true,
      },
      { ...HEALTH[0], errors: { errors: 1, records: 100, percent: 1 } },
    ];
    const { container } = await renderRoute({
      path: PATH,
      answer: (path) => {
        if (path.endsWith("/pipeline-health")) return jsonResponse({ items: losing });
        if (path.endsWith("/pipelines")) return jsonResponse(list(losing.map((h) => pipeline(h.pipeline))));
        return undefined;
      },
    });
    await openHealthTab();
    const table = await screen.findByRole("table", { name: en.pipelines.health.caption });
    const [hsl, detva] = within(table).getAllByRole("row").slice(1);
    expect(within(hsl).getByText(en.pipelines.health.states.failing)).toBeInTheDocument();
    expect(within(hsl).getByText("10 % failed in the last hour (100 of 1,000)")).toHaveClass("text-danger");
    expect(within(hsl).getByText(en.pipelines.health.alerted)).toBeInTheDocument();
    expect(within(detva).getByText("1 % failed in the last hour (1 of 100)")).not.toHaveClass("text-danger");
    await expectAxeClean(container);
  });

  it("is_not_offered_to_a_person_who_approves_no_pipeline", async () => {
    await renderRoute({
      path: PATH,
      permissions: { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read"] } }] },
      answer: answering(() => jsonResponse({ items: HEALTH })),
    });
    await screen.findByRole("tab", { name: en.pipelines.map.view.list });
    await screen.findByText("podujatia-zvolen");
    expect(screen.queryByRole("tab", { name: en.pipelines.map.view.health })).toBeNull();
  });

  it("says_what_to_do_in_a_project_without_pipelines", async () => {
    await renderRoute({ path: PATH, answer: answering(() => jsonResponse({ items: [] })) });
    await openHealthTab();
    expect(await screen.findByText(en.pipelines.health.empty)).toBeInTheDocument();
    expect(screen.getByText(en.pipelines.health.emptyHint)).toBeInTheDocument();
  });

  it("shows_the_server_s_refusal_in_words", async () => {
    const said = "the pipelines' health is for stewards: it needs approve on Pipeline in project helsinki";
    await renderRoute({ path: PATH, answer: answering(() => problem(403, said)) });
    await openHealthTab();
    expect(await screen.findByRole("alert")).toHaveTextContent(said);
  });

  it("orders_alerted_sources_before_the_rest", () => {
    const sorted = byUrgency([
      { pipeline: "b", state: "drift", failedRuns: 0, alert: false },
      { pipeline: "a", state: "ok", failedRuns: 0, alert: false },
      { pipeline: "c", state: "failing", failedRuns: 1, alert: false },
      { pipeline: "d", state: "drift", failedRuns: 0, alert: true },
    ]);
    expect(sorted.map((h) => h.pipeline)).toEqual(["d", "c", "b", "a"]);
  });
});
