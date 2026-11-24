/**
 * T-7680: what a pipeline's own page does beyond showing it. Pause proposes the manifest with
 * `enabled: false` after a green check and shows the Change; a red check writes nothing and says
 * why; a pipeline that cannot be read offers a retry; Edit opens the editor; the target Endpoint,
 * its space and a stalled stream show; the rejected records open in a dialog that leads to their
 * run; a resident runner's totals are polled.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LastRunCell, nextRunOf, periodSeconds } from "../src/pages/pipelines/PipelineOverview";
import { InRouter, jsonResponse, list, renderRoute } from "./pageHarness";

const NOW = Date.parse("2026-10-20T12:00:00Z");
const PATH = "/projects/helsinki/pipelines/air-quality";

const RUN = { run: "2026-10-20T11:00:00Z", firstAt: "2026-10-20T11:55:00Z", lastAt: "2026-10-20T11:56:00Z", sent: 120, rejected: 1, failed: 0 };

const pipeline = (spec: Record<string, unknown>, status: Record<string, unknown> = { phase: "Live" }) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "air-quality", namespace: "helsinki", title: "Air quality" },
  spec: { schedule: "0 * * * *", source: { dataSourceRef: { name: "fmi-feed" } }, output: { type: "AirQualityObserved" }, ...spec },
  status,
});

const REJECTED = {
  items: [{ id: 9, at: "2026-10-20T11:56:00Z", run: RUN.run, rule: "sh:datatype", path: "pm10", message: "pm10 is not of the slot's datatype", record: { pm10: "high" } }],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

const EDITOR = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "editor", binding: "editors", rule: { kinds: ["Pipeline", "Endpoint", "ContextSpace"], verbs: ["read", "propose"] } }],
};

interface Options {
  manifest?: unknown;
  check?: unknown;
  extra?: (path: string, request: Request) => Response | undefined;
}

function answers({ manifest = pipeline({}), check = { valid: true, verdict: { ok: true, findings: [] } }, extra }: Options) {
  return (path: string, request: Request): Response | undefined => {
    const own = extra?.(path, request);
    if (own) return own;
    if (request.method === "PUT" && path.endsWith("/pipelines/air-quality")) {
      return new URL(request.url).searchParams.get("dryRun") === "All" ? jsonResponse(check) : jsonResponse(CHANGE, 202);
    }
    if (path === "/api/v1/projects/helsinki/pipelines/air-quality") return jsonResponse(manifest);
    if (path.endsWith("/air-quality/runs")) return jsonResponse({ items: [RUN] });
    if (path.endsWith("/air-quality/rejected")) return jsonResponse(REJECTED);
    if (path.endsWith("/air-quality/rejected/groups")) return jsonResponse({ groups: [], total: 0 });
    return undefined;
  };
}

const writes = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method === "PUT");

describe("a pipeline's own page, acting", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it("pauses through a checked proposal and shows the Change it made", async () => {
    const page = await renderRoute({ path: PATH, answer: answers({}), permissions: EDITOR });
    const pause = await screen.findByRole("button", { name: en.pipelines.pause });
    await waitFor(() => expect(pause).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(pause);
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
    const [checked, written] = writes(page.fetchMock);
    expect(new URL(checked.url).searchParams.get("dryRun")).toBe("All");
    expect(new URL(written.url).searchParams.get("dryRun")).toBeNull();
    const body = (await written.clone().json()) as { spec: { enabled: boolean }; status?: unknown };
    expect(body.spec.enabled).toBe(false);
    expect(body.status).toBeUndefined();
  });

  it("resumes a paused pipeline, and a red check writes nothing and says what it found", async () => {
    const page = await renderRoute({
      path: PATH,
      answer: answers({
        manifest: pipeline({ enabled: false }),
        check: { valid: false, verdict: { ok: false, findings: [{ message: "the runner has no room for another stream" }] } },
      }),
      permissions: EDITOR,
    });
    const resume = await screen.findByRole("button", { name: en.pipelines.resume });
    await waitFor(() => expect(resume).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(resume);
    expect(await screen.findByText(/the runner has no room for another stream/)).toBeInTheDocument();
    expect(writes(page.fetchMock)).toHaveLength(1);
  });

  it("says a write refused for another reason in plain words", async () => {
    await renderRoute({
      path: PATH,
      answer: answers({
        extra: (_path, request) =>
          request.method === "PUT" ? jsonResponse({ type: "about:blank", title: "Forbidden", status: 403, detail: "you may not propose Pipelines here" }, 403) : undefined,
      }),
      permissions: EDITOR,
    });
    const pause = await screen.findByRole("button", { name: en.pipelines.pause });
    await waitFor(() => expect(pause).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(pause);
    expect(await screen.findByText("you may not propose Pipelines here")).toBeInTheDocument();
  });

  it("offers a retry when the pipeline cannot be read, and shows it once it can", async () => {
    let refuse = true;
    const page = await renderRoute({
      path: PATH,
      answer: answers({
        extra: (path) =>
          refuse && path === "/api/v1/projects/helsinki/pipelines/air-quality"
            ? jsonResponse({ type: "about:blank", title: "Unavailable", status: 503, detail: "the repository is not reachable" }, 503)
            : undefined,
      }),
      permissions: EDITOR,
    });
    const retry = await screen.findByRole("button", { name: en.app.error.retry });
    refuse = false;
    await userEvent.click(retry);
    expect(await screen.findByRole("heading", { level: 1, name: "Air quality" })).toBeInTheDocument();
    expect(page.calls().filter((call) => call === "GET /api/v1/projects/helsinki/pipelines/air-quality").length).toBeGreaterThanOrEqual(2);
  });

  it("opens the editor from Edit", async () => {
    await renderRoute({ path: PATH, answer: answers({}), permissions: EDITOR });
    const edit = await screen.findByRole("button", { name: en.pipelines.edit });
    await waitFor(() => expect(edit).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(edit);
    await waitFor(() => expect(window.location.pathname).toBe(`${PATH}/edit`));
  });

  it("links the target Endpoint and its space, and says a stream that writes nothing has stalled", async () => {
    const target = "urn:ngsi-ld:Endpoint:hel.fi:air:air-writer";
    const stalled = "the stream took 21000 records in and sent none of them";
    await renderRoute({
      path: PATH,
      answer: answers({
        manifest: pipeline(
          { targetEndpoint: target },
          { phase: "Live", conditions: [{ type: "Scheduled", status: "True" }, { type: "StreamWriting", status: "False", reason: "Stalled", message: stalled }] },
        ),
        extra: (path) =>
          path === "/api/v1/projects/helsinki/endpoints"
            ? jsonResponse(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "air-writer", namespace: "helsinki" }, spec: { contextSpaceRef: "air" } }]))
            : undefined,
      }),
      permissions: EDITOR,
    });
    expect(await screen.findByRole("link", { name: "air-writer" })).toHaveAttribute("href", "/projects/helsinki/endpoints/air-writer");
    expect(screen.getByRole("link", { name: "air" })).toHaveAttribute("href", "/projects/helsinki/spaces/air");
    expect(screen.getByText(new RegExp(stalled))).toBeInTheDocument();
  });

  it("shows a target it cannot match to an Endpoint as its URN", async () => {
    const target = "urn:ngsi-ld:Endpoint:hel.fi:gone:old-writer";
    await renderRoute({ path: PATH, answer: answers({ manifest: pipeline({ targetEndpoint: target }) }), permissions: EDITOR });
    expect(await screen.findByText(target)).toBeInTheDocument();
  });

  it("opens every rejected record in a dialog, closes it, and leads from a record to its run", async () => {
    await renderRoute({ path: PATH, answer: answers({}), permissions: EDITOR });
    await screen.findByText("pm10 is not of the slot's datatype");
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.rejected.open }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(dialog).not.toBeInTheDocument());

    await userEvent.click(screen.getByRole("button", { name: en.pipelines.rejected.open }));
    const again = await screen.findByRole("dialog");
    await userEvent.click(await within(again).findByRole("button", { name: en.pipelines.rejected.openRun }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("polls a resident runner's totals and reads a runner that does not answer as none", async () => {
    const page = await renderRoute({
      path: PATH,
      answer: answers({
        manifest: pipeline({ schedule: undefined, class: "resident" }),
        extra: (path) => (path.endsWith("/air-quality/metrics") ? jsonResponse({ type: "about:blank", title: "Unavailable", status: 503 }, 503) : undefined),
      }),
      permissions: EDITOR,
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Air quality" })).toBeInTheDocument();
    await waitFor(() => expect(page.calls()).toContain("GET /api/v1/projects/helsinki/pipelines/air-quality/metrics"));
    expect(screen.queryByText(/Since the runner started:/)).toBeNull();
  });

  it("fills the list's Last run cell from one request for every pipeline", async () => {
    await renderRoute({
      path: "/projects/helsinki/pipelines",
      answer: (path, request) =>
        path === "/api/v1/projects/helsinki/pipelines"
          ? jsonResponse(list([pipeline({})]))
          : path.endsWith("/helsinki/pipeline-runs")
            ? jsonResponse({ items: [{ pipeline: "air-quality", run: { ...RUN, rejected: 0 } }] })
            : answers({})(path, request),
    });
    expect(await screen.findByText("120 written")).toBeInTheDocument();
  });
});

describe("the Last run cell and the next run", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("says it is loading while the runs are on their way", async () => {
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <LastRunCell project="helsinki" name="air-quality" run={undefined} now={NOW} />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(within(await screen.findByTestId("pipeline-last-run")).getByText(en.app.loading)).toBeInTheDocument();
  });

  it("knows no next run for a period it cannot read or a pipeline that never ran", () => {
    expect(periodSeconds("every hour")).toBeUndefined();
    expect(periodSeconds(undefined)).toBeUndefined();
    expect(periodSeconds("250ms")).toBe(0.25);
    expect(nextRunOf({ period: "1h", class: "scheduled" }, true, null, NOW)).toEqual({ kind: "unknown" });
  });
});
