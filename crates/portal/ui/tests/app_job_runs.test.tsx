/**
 * A wasm App's scheduled jobs and their last runs, on its App page (T-3372, AP-154, AP-155): each
 * declared job with its schedule, when it last ran and how, in words beside the colour, the job's
 * own sentence for a run that failed, and a job that never ran said so. An App without jobs shows
 * nothing.
 */
import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppJobRuns, jobsOf } from "../src/pages/apps/AppJobRuns";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";
import { narrowsAndOrders } from "./grid_checks";

const APP = "/api/v1/projects/helsinki/apps/kpi";

function manifest(jobs: unknown[], runs?: unknown[]) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { name: "kpi", namespace: "helsinki" },
    spec: { kind: "wasm", server: { jobs } },
    status: { phase: "Live", ...(runs ? { jobs: runs } : {}) },
  };
}

function renderRuns(app: unknown) {
  return renderPage(<AppJobRuns project="helsinki" name="kpi" />, {
    path: "/projects/helsinki/apps/kpi",
    answer: (url) =>
      url.pathname === APP
        ? new Response(JSON.stringify(app), { status: 200, headers: { "Content-Type": "application/json" } })
        : undefined,
  });
}

const HOURLY = { name: "hourly", schedule: "0 * * * *", export: "compute" };
const NIGHTLY = { name: "nightly", schedule: "15 2 * * *", export: "rollup" };

describe("AppJobRuns", () => {
  it("lists each job with its last run, its outcome in words and a failure's own sentence", async () => {
    const { container } = renderRuns(
      manifest(
        [HOURLY, NIGHTLY],
        [
          { name: "hourly", lastRun: "2026-10-10T14:00:00Z", outcome: "succeeded", durationMs: 42, failuresInARow: 0 },
          {
            name: "nightly",
            lastRun: "2026-10-10T02:15:00Z",
            outcome: "failed",
            message: "403 from the Endpoint: the indicator type is not granted",
            durationMs: 120,
            failuresInARow: 3,
          },
        ],
      ),
    );
    const table = await screen.findByRole("table", { name: "Scheduled jobs of kpi and their last runs" });
    const rows = within(table).getAllByRole("row");
    expect(within(rows[1]).getByText("hourly")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Succeeded")).toBeInTheDocument();
    expect(within(rows[1]).getByText("42 ms")).toBeInTheDocument();
    expect(within(rows[1]).getByText(/UTC$/)).toBeInTheDocument();
    expect(within(rows[2]).getByText("Ended with an error")).toBeInTheDocument();
    expect(within(rows[2]).getByText("403 from the Endpoint: the indicator type is not granted")).toBeInTheDocument();
    expect(within(rows[2]).getByText("3 failed runs in a row")).toBeInTheDocument();
    await expectNoViolations(container);
  });

  it("searches, sorts by duration and filters by outcome on the shared grid (T-6205)", async () => {
    const WEEKLY = { name: "weekly", schedule: "0 3 * * 1", export: "archive" };
    renderRuns(
      manifest(
        [HOURLY, NIGHTLY, WEEKLY],
        [
          { name: "hourly", lastRun: "2026-10-10T14:00:00Z", outcome: "succeeded", durationMs: 42, failuresInARow: 0 },
          { name: "nightly", lastRun: "2026-10-10T02:15:00Z", outcome: "failed", durationMs: 120, failuresInARow: 1 },
          { name: "weekly", lastRun: "2026-10-05T03:00:00Z", outcome: "succeeded", durationMs: 7, failuresInARow: 0 },
        ],
      ),
    );
    const table = await screen.findByRole("table", { name: "Scheduled jobs of kpi and their last runs" });
    await narrowsAndOrders(table, {
      search: { label: "Search Scheduled jobs", text: "night", kept: ["nightly"] },
      sort: { column: "Duration", ascending: ["weekly", "hourly", "nightly"] },
      filter: { value: /^Ended with an error/, kept: ["nightly"] },
    });
  });

  it("says a job that has not run yet, rather than leaving a blank", async () => {
    renderRuns(manifest([HOURLY]));
    const table = await screen.findByRole("table");
    expect(within(table).getAllByText("Not run yet").length).toBeGreaterThan(0);
  });

  it("shows nothing for an App without jobs", async () => {
    const { container } = renderRuns(manifest([]));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(container.querySelector("table")).toBeNull();
  });
});

describe("jobsOf", () => {
  it("keeps only well-formed jobs and runs, and reads a manifest without either as none", () => {
    expect(jobsOf(undefined)).toEqual({ jobs: [], runs: [] });
    const { jobs, runs } = jobsOf(manifest([HOURLY, { name: 7 }], [{ name: "hourly" }]));
    expect(jobs.map((job) => job.name)).toEqual(["hourly"]);
    expect(runs).toEqual([]);
  });
});
